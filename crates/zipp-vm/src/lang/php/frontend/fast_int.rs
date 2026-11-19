//! Pure-integer JIT companion compiler for PHP functions.
//!
//! When a PHP function operates exclusively on integer parameters/locals with
//! integer arithmetic (`+`, `-`, `*`, unary `+/-`, `++`/`--`), comparisons,
//! control flow (`if`/`while`/`do`/`for`/`break`/`continue`/`return`), and
//! optional direct self-recursion, we emit a side-effect-free JS-`Number`
//! companion `FuncProto`:
//!
//! * Self-recursive functions (e.g. `fib`) use `i32` bounds (`±(2^30 - 1)`) and
//!   a dedicated `name_global` slot so ZIPP's Tier-A native x86-64/AArch64
//!   self-call JIT (`codegen/fn_int.rs`) compiles them directly to machine code.
//! * Loop functions (e.g. `sumLoop`) use `±(2^52 - 1)` exact-IEEE-754 bounds so
//!   ZIPP's Tier-C region JIT compiles them to native machine code while staying
//!   mathematically bit-exact with PHP 64-bit integers.
//!
//! Soundness guarantee (IEEE-754 `< 2^52` invariant):
//! * Every argument is guarded at entry to `[-bound, +bound]`.
//! * Every `+`, `-`, `*`, unary `-`, `++`, `--` result `r` is guarded via
//!   `JumpIfNotLe(-bound, r, bail)` and `JumpIfNotLe(r, +bound, bail)` — which
//!   also catches `NaN` / `undefined` from any bailed recursive call.
//! * Because admitted functions have zero observable side effects (no I/O, no
//!   globals, no heap mutations, no external calls), returning `undefined` on
//!   any out-of-range intermediate lets the caller transparently re-run the
//!   exact 64-bit `BigInt` PHP function body with 100% PHP 8.4 parity.

use super::emitter::{FuncSink, R, MAX_INSTRUCTIONS};
use super::resolve::NsResolver;
use super::symtable::{is_superglobal, FnScopeInfo};
use crate::bytecode::{Instr, Reg};
use crate::value::Value;
use std::collections::{HashMap, HashSet};
use zipp_phpparse::ast::*;

/// Tier-A self-call exactness bound (`2^30 - 1024`): fits in `LoadInt` (`i32`) so
/// `codegen/fn_int.rs` admits the whole function, and small `±16` steps plus `a ± b`
/// never overflow `i32`.
pub(super) const TIER_A_BOUND: i64 = (1i64 << 30) - 1024;

/// Tier-C loop exactness bound (`2^52 - 1`): `a ± b` is `< 2^53` (bit-exact in
/// IEEE-754 `f64`), and any `a * b` whose true magnitude is `>= 2^52` rounds to
/// `>= 2^52` and is caught before precision can ever be lost.
pub(super) const TIER_C_BOUND: i64 = (1i64 << 52) - 1;

pub(super) struct FastIntCompiled {
    pub func_id: u32,
    pub self_global_slot: Option<u32>,
    pub bound: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValKind {
    Int,
    Bool,
}

struct FastLoop {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

struct FastIntBuilder {
    name: String,
    fqn_lo: String,
    ns: NsResolver,
    use_i32_bound: bool,
    self_global_slot: Option<u32>,
    param_count: u16,
    code: Vec<Instr>,
    constants: Vec<Value>,
    float_consts: HashMap<u64, u32>,
    next_reg: Reg,
    max_reg: Reg,
    locals: HashMap<String, Reg>,
    assigned: HashSet<String>,
    loops: Vec<FastLoop>,
    bails: Vec<usize>,
    has_self_call: bool,
}

impl FastIntBuilder {
    fn alloc(&mut self) -> R<Reg> {
        let r = self.next_reg;
        if r >= u16::MAX - 8 {
            return Err("fast_int: register limit".into());
        }
        self.next_reg += 1;
        if self.next_reg > self.max_reg {
            self.max_reg = self.next_reg;
        }
        Ok(r)
    }

    fn alloc_block(&mut self, n: u16) -> R<Reg> {
        let base = self.next_reg;
        if base as u32 + n as u32 >= u16::MAX as u32 - 8 {
            return Err("fast_int: register limit".into());
        }
        self.next_reg += n;
        if self.next_reg > self.max_reg {
            self.max_reg = self.next_reg;
        }
        Ok(base)
    }

    fn emit(&mut self, instr: Instr) -> R<usize> {
        if self.code.len() >= MAX_INSTRUCTIONS {
            return Err("fast_int: instruction limit".into());
        }
        let idx = self.code.len();
        self.code.push(instr);
        Ok(idx)
    }

    fn here(&self) -> usize {
        self.code.len()
    }

    fn patch(&mut self, at: usize, target: usize) {
        let t = target as u32;
        match &mut self.code[at] {
            Instr::Jump { target }
            | Instr::JumpIfFalse { target, .. }
            | Instr::JumpIfTrue { target, .. }
            | Instr::JumpIfNotLt { target, .. }
            | Instr::JumpIfNotLe { target, .. } => *target = t,
            _ => {}
        }
    }

    fn load_f64(&mut self, v: f64) -> R<Reg> {
        let bits = v.to_bits();
        let idx = match self.float_consts.get(&bits) {
            Some(&i) => i,
            None => {
                let i = self.constants.len() as u32;
                self.constants.push(Value::num(v));
                self.float_consts.insert(bits, i);
                i
            }
        };
        let dst = self.alloc()?;
        self.emit(Instr::LoadConst { dst, idx })?;
        Ok(dst)
    }

    fn load_bound_neg(&mut self) -> R<Reg> {
        if self.use_i32_bound {
            let dst = self.alloc()?;
            self.emit(Instr::LoadInt {
                dst,
                val: -(TIER_A_BOUND as i32),
            })?;
            Ok(dst)
        } else {
            self.load_f64(-(TIER_C_BOUND as f64))
        }
    }

    fn load_bound_pos(&mut self) -> R<Reg> {
        if self.use_i32_bound {
            let dst = self.alloc()?;
            self.emit(Instr::LoadInt {
                dst,
                val: TIER_A_BOUND as i32,
            })?;
            Ok(dst)
        } else {
            self.load_f64(TIER_C_BOUND as f64)
        }
    }

    /// Emit `if (!(lo <= reg && reg <= hi)) return undefined;`
    /// Also catches `NaN` / `undefined` because `lo <= NaN` is `false`.
    fn emit_range_guard(&mut self, reg: Reg) -> R<()> {
        let saved = self.next_reg;
        let lo = self.load_bound_neg()?;
        let j1 = self.emit(Instr::JumpIfNotLe {
            a: lo,
            b: reg,
            target: 0,
        })?;
        self.bails.push(j1);
        let hi = self.load_bound_pos()?;
        let j2 = self.emit(Instr::JumpIfNotLe {
            a: reg,
            b: hi,
            target: 0,
        })?;
        self.bails.push(j2);
        self.next_reg = saved;
        Ok(())
    }

    fn is_self_name(&self, name: &Name) -> bool {
        let (fqn, fb) = self.ns.resolve_function(name);
        if fqn.trim_start_matches('\\').eq_ignore_ascii_case(&self.fqn_lo) {
            return true;
        }
        if let Some(g) = fb {
            if g.trim_start_matches('\\').eq_ignore_ascii_case(&self.fqn_lo) {
                return true;
            }
        }
        false
    }

    fn emit_stmts(&mut self, stmts: &[Stmt]) -> R<bool> {
        for s in stmts {
            if !self.emit_stmt(s)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> R<bool> {
        match &stmt.kind {
            StmtKind::Nop => Ok(true),
            StmtKind::Block(body) => self.emit_stmts(body),
            StmtKind::Expr(e) => {
                let saved = self.next_reg;
                let ok = self.emit_expr(e)?.is_some();
                self.next_reg = saved;
                Ok(ok)
            }
            StmtKind::Return(Some(e)) => {
                let saved = self.next_reg;
                let Some((r, kind)) = self.emit_expr(e)? else {
                    return Ok(false);
                };
                if kind != ValKind::Int {
                    return Ok(false);
                }
                self.emit(Instr::Return { src: r })?;
                self.next_reg = saved;
                Ok(true)
            }
            StmtKind::If {
                cond,
                then_body,
                elseifs,
                else_body,
            } => {
                let saved_assigned = self.assigned.clone();
                let mut end_jumps = Vec::new();
                let saved = self.next_reg;
                let Some(j_false) = self.emit_jump_if_false(cond)? else {
                    return Ok(false);
                };
                self.next_reg = saved;
                if !self.emit_stmts(then_body)? {
                    return Ok(false);
                }
                end_jumps.push(self.emit(Instr::Jump { target: 0 })?);
                let mut next_target = j_false;

                for (ei_cond, ei_body) in elseifs {
                    self.assigned = saved_assigned.clone();
                    let here = self.here();
                    self.patch(next_target, here);
                    let saved = self.next_reg;
                    let Some(jf) = self.emit_jump_if_false(ei_cond)? else {
                        return Ok(false);
                    };
                    self.next_reg = saved;
                    if !self.emit_stmts(ei_body)? {
                        return Ok(false);
                    }
                    end_jumps.push(self.emit(Instr::Jump { target: 0 })?);
                    next_target = jf;
                }

                let here = self.here();
                self.patch(next_target, here);
                if let Some(eb) = else_body {
                    self.assigned = saved_assigned.clone();
                    if !self.emit_stmts(eb)? {
                        return Ok(false);
                    }
                }
                let end = self.here();
                for j in end_jumps {
                    self.patch(j, end);
                }
                self.assigned = saved_assigned;
                Ok(true)
            }
            StmtKind::While { cond, body } => {
                let saved_assigned = self.assigned.clone();
                let head = self.here();
                self.loops.push(FastLoop {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                let saved = self.next_reg;
                let Some(exit) = self.emit_jump_if_false(cond)? else {
                    return Ok(false);
                };
                self.next_reg = saved;
                if !self.emit_stmts(body)? {
                    return Ok(false);
                }
                let lp = self.loops.pop().unwrap();
                for c in lp.continues {
                    self.patch(c, head);
                }
                self.emit(Instr::Jump {
                    target: head as u32,
                })?;
                let end = self.here();
                self.patch(exit, end);
                for b in lp.breaks {
                    self.patch(b, end);
                }
                self.assigned = saved_assigned;
                Ok(true)
            }
            StmtKind::DoWhile { body, cond } => {
                let saved_assigned = self.assigned.clone();
                let head = self.here();
                self.loops.push(FastLoop {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                if !self.emit_stmts(body)? {
                    return Ok(false);
                }
                let cond_ip = self.here();
                let lp = self.loops.pop().unwrap();
                for c in lp.continues {
                    self.patch(c, cond_ip);
                }
                let saved = self.next_reg;
                let Some((cr, _)) = self.emit_expr(cond)? else {
                    return Ok(false);
                };
                self.emit(Instr::JumpIfTrue {
                    cond: cr,
                    target: head as u32,
                })?;
                self.next_reg = saved;
                let end = self.here();
                for b in lp.breaks {
                    self.patch(b, end);
                }
                self.assigned = saved_assigned;
                Ok(true)
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                for e in init {
                    let saved = self.next_reg;
                    if self.emit_expr(e)?.is_none() {
                        return Ok(false);
                    }
                    self.next_reg = saved;
                }
                let saved_assigned = self.assigned.clone();
                let head = self.here();
                let mut exit = None;
                if !cond.is_empty() {
                    for e in &cond[..cond.len() - 1] {
                        let saved = self.next_reg;
                        if self.emit_expr(e)?.is_none() {
                            return Ok(false);
                        }
                        self.next_reg = saved;
                    }
                    let saved = self.next_reg;
                    let Some(j) = self.emit_jump_if_false(cond.last().unwrap())? else {
                        return Ok(false);
                    };
                    self.next_reg = saved;
                    exit = Some(j);
                }
                self.loops.push(FastLoop {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                if !self.emit_stmts(body)? {
                    return Ok(false);
                }
                let step_ip = self.here();
                let lp = self.loops.pop().unwrap();
                for c in lp.continues {
                    self.patch(c, step_ip);
                }
                for e in step {
                    let saved = self.next_reg;
                    if self.emit_expr(e)?.is_none() {
                        return Ok(false);
                    }
                    self.next_reg = saved;
                }
                self.emit(Instr::Jump {
                    target: head as u32,
                })?;
                let end = self.here();
                if let Some(ex) = exit {
                    self.patch(ex, end);
                }
                for b in lp.breaks {
                    self.patch(b, end);
                }
                self.assigned = saved_assigned;
                Ok(true)
            }
            StmtKind::Break(lvl_expr) => {
                let lvl = match lvl_expr {
                    None => 1usize,
                    Some(Expr {
                        kind: ExprKind::Int(n),
                        ..
                    }) if *n >= 1 => *n as usize,
                    _ => return Ok(false),
                };
                if lvl > self.loops.len() {
                    return Ok(false);
                }
                let j = self.emit(Instr::Jump { target: 0 })?;
                let idx = self.loops.len() - lvl;
                self.loops[idx].breaks.push(j);
                Ok(true)
            }
            StmtKind::Continue(lvl_expr) => {
                let lvl = match lvl_expr {
                    None => 1usize,
                    Some(Expr {
                        kind: ExprKind::Int(n),
                        ..
                    }) if *n >= 1 => *n as usize,
                    _ => return Ok(false),
                };
                if lvl > self.loops.len() {
                    return Ok(false);
                }
                let j = self.emit(Instr::Jump { target: 0 })?;
                let idx = self.loops.len() - lvl;
                self.loops[idx].continues.push(j);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Emit a fused conditional branch (`JumpIfNotLt` / `JumpIfNotLe` / `JumpIfFalse`)
    /// so Tier-A and Tier-C JITs fuse compare+branch into a single x86-64 `cmp + jcc`!
    fn emit_jump_if_false(&mut self, cond: &Expr) -> R<Option<usize>> {
        if let ExprKind::Binary { op, left, right } = &cond.kind {
            if matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq) {
                let Some((a, ka)) = self.emit_expr(left)? else {
                    return Ok(None);
                };
                let Some((b, kb)) = self.emit_expr(right)? else {
                    return Ok(None);
                };
                if ka != ValKind::Int || kb != ValKind::Int {
                    return Ok(None);
                }
                let j = match op {
                    BinOp::Lt => self.emit(Instr::JumpIfNotLt { a, b, target: 0 })?,
                    BinOp::LtEq => self.emit(Instr::JumpIfNotLe { a, b, target: 0 })?,
                    BinOp::Gt => self.emit(Instr::JumpIfNotLt {
                        a: b,
                        b: a,
                        target: 0,
                    })?,
                    BinOp::GtEq => self.emit(Instr::JumpIfNotLe {
                        a: b,
                        b: a,
                        target: 0,
                    })?,
                    _ => unreachable!(),
                };
                return Ok(Some(j));
            }
        }
        let Some((c, _)) = self.emit_expr(cond)? else {
            return Ok(None);
        };
        let j = self.emit(Instr::JumpIfFalse { cond: c, target: 0 })?;
        Ok(Some(j))
    }

    fn emit_expr(&mut self, expr: &Expr) -> R<Option<(Reg, ValKind)>> {
        match &expr.kind {
            ExprKind::Int(v) => {
                let bound = if self.use_i32_bound {
                    TIER_A_BOUND
                } else {
                    TIER_C_BOUND
                };
                if *v < -bound || *v > bound {
                    return Ok(None);
                }
                if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                    let dst = self.alloc()?;
                    self.emit(Instr::LoadInt {
                        dst,
                        val: *v as i32,
                    })?;
                    Ok(Some((dst, ValKind::Int)))
                } else if !self.use_i32_bound {
                    let dst = self.load_f64(*v as f64)?;
                    Ok(Some((dst, ValKind::Int)))
                } else {
                    Ok(None)
                }
            }
            ExprKind::Var(name) => {
                if !self.assigned.contains(name) {
                    return Ok(None);
                }
                let Some(&reg) = self.locals.get(name) else {
                    return Ok(None);
                };
                Ok(Some((reg, ValKind::Int)))
            }
            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Plus => {
                    let Some((r, ValKind::Int)) = self.emit_expr(inner)? else {
                        return Ok(None);
                    };
                    Ok(Some((r, ValKind::Int)))
                }
                UnaryOp::Neg => {
                    let Some((r, ValKind::Int)) = self.emit_expr(inner)? else {
                        return Ok(None);
                    };
                    let z = self.alloc()?;
                    self.emit(Instr::LoadInt { dst: z, val: 0 })?;
                    let dst = self.alloc()?;
                    self.emit(Instr::Sub { dst, a: z, b: r })?;
                    self.emit_range_guard(dst)?;
                    Ok(Some((dst, ValKind::Int)))
                }
                UnaryOp::Not => {
                    let Some((r, _)) = self.emit_expr(inner)? else {
                        return Ok(None);
                    };
                    let z = self.alloc()?;
                    self.emit(Instr::LoadInt { dst: z, val: 0 })?;
                    let dst = self.alloc()?;
                    self.emit(Instr::Eq { dst, a: r, b: z })?;
                    Ok(Some((dst, ValKind::Bool)))
                }
                UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
                    let ExprKind::Var(name) = &inner.kind else {
                        return Ok(None);
                    };
                    if !self.assigned.contains(name) {
                        return Ok(None);
                    }
                    let Some(&vreg) = self.locals.get(name) else {
                        return Ok(None);
                    };
                    let imm = match op {
                        UnaryOp::PreInc | UnaryOp::PostInc => 1i32,
                        _ => -1i32,
                    };
                    let is_post = matches!(op, UnaryOp::PostInc | UnaryOp::PostDec);
                    let res = if is_post {
                        let old = self.alloc()?;
                        self.emit(Instr::Move { dst: old, src: vreg })?;
                        old
                    } else {
                        vreg
                    };
                    self.emit(Instr::AddInt {
                        dst: vreg,
                        a: vreg,
                        imm,
                        upd: true,
                    })?;
                    self.emit_range_guard(vreg)?;
                    Ok(Some((res, ValKind::Int)))
                }
                _ => Ok(None),
            },
            ExprKind::Assign { op, target, value } => {
                let ExprKind::Var(name) = &target.kind else {
                    return Ok(None);
                };
                let Some(&vreg) = self.locals.get(name) else {
                    return Ok(None);
                };
                match op {
                    AssignOp::Assign => {
                        let Some((vr, ValKind::Int)) = self.emit_expr(value)? else {
                            return Ok(None);
                        };
                        if vr != vreg {
                            self.emit(Instr::Move { dst: vreg, src: vr })?;
                        }
                        self.assigned.insert(name.clone());
                        Ok(Some((vreg, ValKind::Int)))
                    }
                    AssignOp::Add | AssignOp::Sub | AssignOp::Mul => {
                        if !self.assigned.contains(name) {
                            return Ok(None);
                        }
                        if *op == AssignOp::Add {
                            if let ExprKind::Int(imm) = value.kind {
                                if imm >= -1_000_000 && imm <= 1_000_000 {
                                    self.emit(Instr::AddInt {
                                        dst: vreg,
                                        a: vreg,
                                        imm: imm as i32,
                                        upd: false,
                                    })?;
                                    self.emit_range_guard(vreg)?;
                                    return Ok(Some((vreg, ValKind::Int)));
                                }
                            }
                        }
                        let Some((vr, ValKind::Int)) = self.emit_expr(value)? else {
                            return Ok(None);
                        };
                        match op {
                            AssignOp::Add => self.emit(Instr::Add {
                                dst: vreg,
                                a: vreg,
                                b: vr,
                            })?,
                            AssignOp::Sub => self.emit(Instr::Sub {
                                dst: vreg,
                                a: vreg,
                                b: vr,
                            })?,
                            AssignOp::Mul => self.emit(Instr::Mul {
                                dst: vreg,
                                a: vreg,
                                b: vr,
                            })?,
                            _ => unreachable!(),
                        };
                        self.emit_range_guard(vreg)?;
                        Ok(Some((vreg, ValKind::Int)))
                    }
                    _ => Ok(None),
                }
            }
            ExprKind::Binary { op, left, right } => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    if *op == BinOp::Add {
                        if let ExprKind::Int(imm) = right.kind {
                            if imm >= -1_000_000 && imm <= 1_000_000 {
                                let Some((a, ValKind::Int)) = self.emit_expr(left)? else {
                                    return Ok(None);
                                };
                                let dst = self.alloc()?;
                                self.emit(Instr::AddInt {
                                    dst,
                                    a,
                                    imm: imm as i32,
                                    upd: false,
                                })?;
                                if !(self.use_i32_bound && (-16..=16).contains(&imm)) {
                                    self.emit_range_guard(dst)?;
                                }
                                return Ok(Some((dst, ValKind::Int)));
                            }
                        }
                    } else if *op == BinOp::Sub {
                        if let ExprKind::Int(imm) = right.kind {
                            if imm >= -1_000_000 && imm <= 1_000_000 {
                                let Some((a, ValKind::Int)) = self.emit_expr(left)? else {
                                    return Ok(None);
                                };
                                let dst = self.alloc()?;
                                self.emit(Instr::AddInt {
                                    dst,
                                    a,
                                    imm: -(imm as i32),
                                    upd: false,
                                })?;
                                if !(self.use_i32_bound && (-16..=16).contains(&imm)) {
                                    self.emit_range_guard(dst)?;
                                }
                                return Ok(Some((dst, ValKind::Int)));
                            }
                        }
                    }
                    let Some((a, ValKind::Int)) = self.emit_expr(left)? else {
                        return Ok(None);
                    };
                    let Some((b, ValKind::Int)) = self.emit_expr(right)? else {
                        return Ok(None);
                    };
                    let dst = self.alloc()?;
                    match op {
                        BinOp::Add => self.emit(Instr::Add { dst, a, b })?,
                        BinOp::Sub => self.emit(Instr::Sub { dst, a, b })?,
                        BinOp::Mul => self.emit(Instr::Mul { dst, a, b })?,
                        _ => unreachable!(),
                    };
                    self.emit_range_guard(dst)?;
                    Ok(Some((dst, ValKind::Int)))
                }
                BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::Eq
                | BinOp::StrictEq
                | BinOp::NotEq
                | BinOp::StrictNotEq => {
                    let Some((a, ValKind::Int)) = self.emit_expr(left)? else {
                        return Ok(None);
                    };
                    let Some((b, ValKind::Int)) = self.emit_expr(right)? else {
                        return Ok(None);
                    };
                    let dst = self.alloc()?;
                    match op {
                        BinOp::Lt => self.emit(Instr::Lt { dst, a, b })?,
                        BinOp::LtEq => self.emit(Instr::Le { dst, a, b })?,
                        BinOp::Gt => self.emit(Instr::Gt { dst, a, b })?,
                        BinOp::GtEq => self.emit(Instr::Ge { dst, a, b })?,
                        BinOp::Eq | BinOp::StrictEq => self.emit(Instr::Eq { dst, a, b })?,
                        BinOp::NotEq | BinOp::StrictNotEq => self.emit(Instr::Ne { dst, a, b })?,
                        _ => unreachable!(),
                    };
                    Ok(Some((dst, ValKind::Bool)))
                }
                _ => Ok(None),
            },
            ExprKind::Call {
                callee: Callee::Name(n),
                args: CallArgs::Normal(list),
            } => {
                let Some(slot) = self.self_global_slot else {
                    return Ok(None);
                };
                if !self.is_self_name(n)
                    || list.len() != self.param_count as usize
                    || list.iter().any(|a| a.unpack || a.name.is_some())
                {
                    return Ok(None);
                }
                self.has_self_call = true;
                let argc = list.len() as u16;
                let arg_base = if argc == 1 {
                    let Some((r, ValKind::Int)) = self.emit_expr(&list[0].value)? else {
                        return Ok(None);
                    };
                    r
                } else {
                    let mut evaluated = Vec::with_capacity(list.len());
                    for a in list {
                        let Some((r, ValKind::Int)) = self.emit_expr(&a.value)? else {
                            return Ok(None);
                        };
                        evaluated.push(r);
                    }
                    let base = self.alloc_block(argc)?;
                    for (i, r) in evaluated.into_iter().enumerate() {
                        self.emit(Instr::Move {
                            dst: base + i as u16,
                            src: r,
                        })?;
                    }
                    base
                };
                let dst = self.alloc()?;
                // `fn_int.rs` requires `LoadGlobal(self_slot)` IMMEDIATELY before `Instr::Call`!
                let callee = self.alloc()?;
                self.emit(Instr::LoadGlobal {
                    dst: callee,
                    idx: slot,
                })?;
                self.emit(Instr::Call {
                    dst,
                    callee,
                    arg_base,
                    argc,
                })?;
                Ok(Some((dst, ValKind::Int)))
            }
            _ => Ok(None),
        }
    }
}

fn stmt_has_self_call(stmts: &[Stmt], fqn_lo: &str, ns: &NsResolver) -> bool {
    for s in stmts {
        match &s.kind {
            StmtKind::Block(b) => {
                if stmt_has_self_call(b, fqn_lo, ns) {
                    return true;
                }
            }
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => {
                if expr_has_self_call(e, fqn_lo, ns) {
                    return true;
                }
            }
            StmtKind::If {
                cond,
                then_body,
                elseifs,
                else_body,
            } => {
                if expr_has_self_call(cond, fqn_lo, ns)
                    || stmt_has_self_call(then_body, fqn_lo, ns)
                    || elseifs.iter().any(|(ei_cond, ei_body)| {
                        expr_has_self_call(ei_cond, fqn_lo, ns)
                            || stmt_has_self_call(ei_body, fqn_lo, ns)
                    })
                    || else_body
                        .as_deref()
                        .is_some_and(|b| stmt_has_self_call(b, fqn_lo, ns))
                {
                    return true;
                }
            }
            StmtKind::While { cond, body } | StmtKind::DoWhile { body, cond } => {
                if expr_has_self_call(cond, fqn_lo, ns) || stmt_has_self_call(body, fqn_lo, ns) {
                    return true;
                }
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                if init.iter().any(|e| expr_has_self_call(e, fqn_lo, ns))
                    || cond.iter().any(|e| expr_has_self_call(e, fqn_lo, ns))
                    || step.iter().any(|e| expr_has_self_call(e, fqn_lo, ns))
                    || stmt_has_self_call(body, fqn_lo, ns)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn expr_has_self_call(expr: &Expr, fqn_lo: &str, ns: &NsResolver) -> bool {
    match &expr.kind {
        ExprKind::Call {
            callee: Callee::Name(n),
            args: CallArgs::Normal(list),
        } => {
            let (fqn, fb) = ns.resolve_function(n);
            if fqn.trim_start_matches('\\').eq_ignore_ascii_case(fqn_lo)
                || fb
                    .as_deref()
                    .is_some_and(|g| g.trim_start_matches('\\').eq_ignore_ascii_case(fqn_lo))
            {
                return true;
            }
            list.iter().any(|a| expr_has_self_call(&a.value, fqn_lo, ns))
        }
        ExprKind::Binary { left, right, .. } => {
            expr_has_self_call(left, fqn_lo, ns) || expr_has_self_call(right, fqn_lo, ns)
        }
        ExprKind::Unary { expr: e, .. } => expr_has_self_call(e, fqn_lo, ns),
        ExprKind::Assign { value, .. } => expr_has_self_call(value, fqn_lo, ns),
        _ => false,
    }
}

fn ret_type_allows_int(ty: &Option<TypeHint>) -> bool {
    match ty {
        None => true,
        Some(TypeHint::Simple(n)) => {
            n.parts.len() == 1
                && matches!(n.parts[0].to_ascii_lowercase().as_str(), "int" | "mixed")
        }
        Some(TypeHint::Nullable(inner)) => ret_type_allows_int(&Some((**inner).clone())),
        Some(TypeHint::Union(items)) => items
            .iter()
            .any(|t| ret_type_allows_int(&Some(t.clone()))),
        _ => false,
    }
}

fn param_type_allows_int(ty: &Option<TypeHint>) -> bool {
    match ty {
        None => true,
        Some(TypeHint::Simple(n)) => {
            n.parts.len() == 1
                && matches!(n.parts[0].to_ascii_lowercase().as_str(), "int" | "mixed")
        }
        Some(TypeHint::Nullable(inner)) => param_type_allows_int(&Some((**inner).clone())),
        Some(TypeHint::Union(items)) => items
            .iter()
            .any(|t| param_type_allows_int(&Some(t.clone()))),
        _ => false,
    }
}

/// Attempt to compile a pure-int native-JIT companion for a PHP function.
/// Returns `Ok(None)` if the function falls outside the pure-int subset.
pub(super) fn try_compile_fast_int(
    fqn: &str,
    ns: &NsResolver,
    decl: &FunctionDecl,
    info: &FnScopeInfo,
    sink: &FuncSink<'_>,
) -> R<Option<FastIntCompiled>> {
    if decl.by_ref
        || info.use_scope_map
        || info.is_generator
        || info.uses_func_args
        || !info.ref_vars.is_empty()
        || decl.params.is_empty()
        || decl.params.len() > 2
        || !ret_type_allows_int(&decl.return_type)
    {
        return Ok(None);
    }
    for p in &decl.params {
        if p.by_ref
            || p.variadic
            || p.default.is_some()
            || is_superglobal(&p.name)
            || !param_type_allows_int(&p.ty)
        {
            return Ok(None);
        }
    }

    let fqn_lo = fqn.trim_start_matches('\\').to_ascii_lowercase();
    let wants_self = stmt_has_self_call(&decl.body, &fqn_lo, ns);
    let self_global_slot = if wants_self {
        match sink.alloc_global(format!("__php_fi_{}_{}", fqn_lo, sink.next_id())) {
            Some(slot) => Some(slot),
            None => return Ok(None),
        }
    } else {
        None
    };

    let mut locals = HashMap::new();
    let mut assigned = HashSet::new();
    // JS calling convention: reg 0 = `this`, reg 1..=param_count = params.
    let mut next_reg: Reg = 1;
    for p in &decl.params {
        locals.insert(p.name.clone(), next_reg);
        assigned.insert(p.name.clone());
        next_reg += 1;
    }
    for vname in &info.locals {
        if !locals.contains_key(vname) && !is_superglobal(vname) && vname != "this" {
            locals.insert(vname.clone(), next_reg);
            next_reg += 1;
        }
    }

    let use_i32_bound = wants_self;
    let mut builder = FastIntBuilder {
        name: format!("__php_fi_{}", fqn),
        fqn_lo,
        ns: ns.clone(),
        use_i32_bound,
        self_global_slot,
        param_count: decl.params.len() as u16,
        code: Vec::new(),
        constants: Vec::new(),
        float_consts: HashMap::new(),
        next_reg,
        max_reg: next_reg,
        locals,
        assigned,
        loops: Vec::new(),
        bails: Vec::new(),
        has_self_call: false,
    };

    if !builder.emit_stmts(&decl.body)? {
        return Ok(None);
    }
    // Fall-off or overflow bail both return `undefined` -> caller transparently runs exact BigInt `fn.code`.
    let bail_ip = builder.here();
    for b in std::mem::take(&mut builder.bails) {
        builder.patch(b, bail_ip);
    }
    builder.emit(Instr::ReturnUndefined)?;

    let mut proto = crate::compile::placeholder(&builder.name);
    proto.param_count = builder.param_count;
    proto.length = builder.param_count;
    proto.reg_count = builder.max_reg.max(builder.param_count + 1);
    proto.code = builder.code;
    proto.constants = builder.constants;
    proto.is_strict = true;
    proto.non_constructable = true;
    proto.simple_params = true;
    if builder.has_self_call {
        proto.name_global = builder.self_global_slot;
    }
    let func_id = sink.push(proto)?;
    Ok(Some(FastIntCompiled {
        func_id,
        self_global_slot: if builder.has_self_call {
            builder.self_global_slot
        } else {
            None
        },
        bound: if use_i32_bound {
            TIER_A_BOUND
        } else {
            TIER_C_BOUND
        },
    }))
}
