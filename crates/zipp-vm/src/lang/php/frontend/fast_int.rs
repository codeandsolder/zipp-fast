//! Pure-JS native-JIT companion compiler for PHP functions and methods.
//!
//! When a PHP function or method operates on integer parameters/locals with
//! integer arithmetic (`+`, `-`, `*`, unary `+/-`, `++`/`--`), comparisons,
//! control flow (`if`/`while`/`do`/`for`/`foreach`/`break`/`continue`/`return`),
//! optional direct self-recursion, pure-ASCII strings (`strlen`, concat/interp),
//! non-escaping local arrays (`[]`, `$a[$k] = $v`, `$a[$k]`, `count($a)`,
//! `foreach ($a as [$k =>] $v)`), or non-escaping local simple-int objects
//! (`new C(...)`, `$c->m(...)`, `$c->prop`), we emit a side-effect-free
//! JS-`Number` companion `FuncProto` named `__php_fi_*`:
//!
//! * Self-recursive pure-int functions (e.g. `fib`) use `i32` bounds
//!   (`±(2^30 - 1024)`) and a dedicated `name_global` slot so ZIPP's Tier-A
//!   native x86-64/AArch64 self-call JIT (`codegen/fn_int.rs`) compiles them
//!   directly to machine code.
//! * Loop / local-array / local-object functions and pure-int methods use
//!   `±(2^52 - 1)` exact IEEE-754 bounds so ZIPP's Tier-C JIT compiles them
//!   to native machine code while staying mathematically bit-exact with PHP
//!   64-bit integers.
//!
//! Soundness guarantee:
//! * Every argument is guarded at entry to `[-bound, +bound]`.
//! * Every `+`, `-`, `*`, unary `-`, `++`, `--`, map-get, and method-call result
//!   `r` is guarded via `JumpIfNotLe(-bound, r, bail)` and
//!   `JumpIfNotLe(r, +bound, bail)` — which also catches `NaN` / `undefined`
//!   from any bailed callee or missing key.
//! * Local arrays only accept uniformly typed keys: either `Int` or statically
//!   proven non-integer ASCII strings (never coerced to integer keys in PHP,
//!   and UTF-8 byte length === UTF-16 `.length`).
//! * Local objects never escape and dynamically verify `cls.fastNumCtor`,
//!   property existence, and method `fnumArity` / `fnumRetInt` at construction.
//! * Because admitted functions have zero observable side effects (no I/O, no
//!   globals, no escaping heap mutations, no external calls), returning
//!   `undefined` on any bail lets the caller transparently re-run the exact
//!   64-bit `BigInt` PHP function body with 100% PHP 8.4 parity.

use super::emitter::{FuncSink, MAX_INSTRUCTIONS, R};
use super::resolve::NsResolver;
use super::symtable::{is_superglobal, FnScopeInfo};
use crate::bytecode::{Instr, Reg};
use crate::value::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use zipp_phpparse::ast::*;

/// Tier-A self-call exactness bound (`2^30 - 1024`): fits in `LoadInt` (`i32`) so
/// `codegen/fn_int.rs` admits the whole function, and small `±16` steps plus `a ± b`
/// never overflow `i32`.
pub(super) const TIER_A_BOUND: i64 = (1i64 << 30) - 1024;

/// Tier-C loop exactness bound (`2^52 - 1024`): `a ± b` is `< 2^53` (bit-exact in
/// IEEE-754 `f64`), small `±16` offsets stay strictly `< 2^52`, and any `a * b`
/// whose true magnitude is `>= 2^52` rounds to `>= 2^52` and is caught before
/// precision can ever be lost.
pub(super) const TIER_C_BOUND: i64 = (1i64 << 52) - 1024;

pub(super) struct FastIntCompiled {
    pub func_id: u32,
    pub self_global_slot: Option<u32>,
    pub bound: i64,
}

pub(super) struct FastMethodCompiled {
    pub func_id: u32,
    pub arity: u16,
    pub returns_int: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MapKeyKind {
    Unknown,
    NonIntAsciiStr,
    Int,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValKind {
    Int,
    Bool,
    AsciiStr { non_int_key: bool, non_empty: bool },
    LocalMap(MapKeyKind),
    LocalObj,
}

impl ValKind {
    fn compatible_with(self, other: ValKind) -> Option<ValKind> {
        match (self, other) {
            (ValKind::Int, ValKind::Int) => Some(ValKind::Int),
            (ValKind::Bool, ValKind::Bool) => Some(ValKind::Bool),
            (
                ValKind::AsciiStr {
                    non_int_key: n1,
                    non_empty: e1,
                },
                ValKind::AsciiStr {
                    non_int_key: n2,
                    non_empty: e2,
                },
            ) => Some(ValKind::AsciiStr {
                non_int_key: n1 && n2,
                non_empty: e1 && e2,
            }),
            (ValKind::LocalMap(a), ValKind::LocalMap(b)) => match (a, b) {
                (MapKeyKind::Unknown, x) | (x, MapKeyKind::Unknown) => Some(ValKind::LocalMap(x)),
                (x, y) if x == y => Some(ValKind::LocalMap(x)),
                _ => None,
            },
            (ValKind::LocalObj, ValKind::LocalObj) => Some(ValKind::LocalObj),
            _ => None,
        }
    }
}

#[derive(Default)]
struct LocalObjUsage {
    /// `(method_name_lo, argc) -> result_used_as_int`
    methods: BTreeMap<(String, usize), bool>,
    props: BTreeSet<String>,
}

struct FastLoop {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

struct FastIntBuilder {
    name: String,
    fqn_lo: String,
    ns: NsResolver,
    rt_slot: u32,
    use_i32_bound: bool,
    self_global_slot: Option<u32>,
    param_count: u16,
    /// Present only when compiling a pure-int class method: allowed `$this->prop` names.
    this_int_props: Option<HashSet<String>>,
    /// True when a `: void` or untyped method may return `0` on `return;` / fall-off.
    allow_void_return: bool,
    code: Vec<Instr>,
    constants: Vec<Value>,
    string_constants: Vec<String>,
    string_ids: HashMap<String, u32>,
    string_consts: HashMap<u32, u32>,
    float_consts: HashMap<u64, u32>,
    next_reg: Reg,
    max_reg: Reg,
    locals: HashMap<String, Reg>,
    var_kinds: HashMap<String, ValKind>,
    assigned: HashSet<String>,
    /// Local variables eligible for non-escaping `LocalObj` specialization.
    local_obj_uses: HashMap<String, LocalObjUsage>,
    /// Map variables currently being iterated by an enclosing `foreach`.
    iterated_maps: HashSet<String>,
    /// Persistent hoisted `(set_fn_reg, get_fn_reg)` allocated BEFORE `emit_stmts`
    /// so `StmtKind::Expr`'s `next_reg` save/restore never recycles them, while
    /// keeping Tier-C's native `captured method lane` active with zero in-loop `GetProp`!
    map_method_regs: Option<(Reg, Reg)>,
    loops: Vec<FastLoop>,
    bails: Vec<usize>,
    has_self_call: bool,
    /// Hoisted `(-bound, +bound)` registers (populated once in prologue for `!use_i32_bound`).
    bound_regs: Option<(Reg, Reg)>,
}

/// True if `s` is a PHP canonical decimal integer array key (e.g. `"0"`, `"42"`, `"-7"`).
fn is_php_canonical_int_str(s: &str) -> bool {
    if s.is_empty() || s.len() > 20 {
        return false;
    }
    if s == "0" {
        return true;
    }
    let bytes = s.as_bytes();
    let start = if bytes[0] == b'-' { 1 } else { 0 };
    if start >= bytes.len() {
        return false;
    }
    if !(b'1'..=b'9').contains(&bytes[start]) {
        return false;
    }
    if !bytes[start + 1..].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    s.parse::<i64>().is_ok()
}

/// True if a non-empty ASCII prefix guarantees any concatenation starting with it
/// can NEVER be a PHP canonical integer string (e.g. starts with `'k'`, `'_'`, etc.).
fn prefix_prevents_php_int_key(s: &str) -> bool {
    match s.as_bytes().first() {
        Some(&b) => b != b'-' && !b.is_ascii_digit(),
        None => false,
    }
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

    fn string_index(&mut self, value: &str) -> u32 {
        if let Some(&idx) = self.string_ids.get(value) {
            return idx;
        }
        let idx = self.string_constants.len() as u32;
        self.string_constants.push(value.to_owned());
        self.string_ids.insert(value.to_owned(), idx);
        idx
    }

    fn load_str(&mut self, value: &str) -> R<Reg> {
        let si = self.string_index(value);
        let cidx = match self.string_consts.get(&si) {
            Some(&i) => i,
            None => {
                let i = self.constants.len() as u32;
                self.constants
                    .push(Value::heap(crate::vm::STRING_CONST_BIT | si));
                self.string_consts.insert(si, i);
                i
            }
        };
        let dst = self.alloc()?;
        self.emit(Instr::LoadConst { dst, idx: cidx })?;
        Ok(dst)
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
        if let Some((lo, hi)) = self.bound_regs {
            let j1 = self.emit(Instr::JumpIfNotLe {
                a: lo,
                b: reg,
                target: 0,
            })?;
            self.bails.push(j1);
            let j2 = self.emit(Instr::JumpIfNotLe {
                a: reg,
                b: hi,
                target: 0,
            })?;
            self.bails.push(j2);
            return Ok(());
        }
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

    fn record_var_kind(&mut self, name: &str, kind: ValKind) -> bool {
        match self.var_kinds.get(name).copied() {
            None => {
                self.var_kinds.insert(name.to_owned(), kind);
                true
            }
            Some(prev) => match prev.compatible_with(kind) {
                Some(merged) => {
                    self.var_kinds.insert(name.to_owned(), merged);
                    true
                }
                None => false,
            },
        }
    }

    fn is_self_name(&self, name: &Name) -> bool {
        if self.fqn_lo.is_empty() {
            return false;
        }
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

    fn is_global_builtin(&self, name: &Name, expected: &str) -> bool {
        let (fqn, fb) = self.ns.resolve_function(name);
        if fqn.trim_start_matches('\\').eq_ignore_ascii_case(expected) {
            return true;
        }
        if let Some(g) = fb {
            if g.trim_start_matches('\\').eq_ignore_ascii_case(expected) {
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
            StmtKind::Return(None) => {
                if !self.allow_void_return {
                    return Ok(false);
                }
                let saved = self.next_reg;
                let z = self.alloc()?;
                self.emit(Instr::LoadInt { dst: z, val: 0 })?;
                self.emit(Instr::Return { src: z })?;
                self.next_reg = saved;
                Ok(true)
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
            StmtKind::ForEach {
                expr,
                by_ref,
                key,
                value,
                body,
            } => {
                if *by_ref || self.use_i32_bound {
                    return Ok(false);
                }
                let ExprKind::Var(mname) = &expr.kind else {
                    return Ok(false);
                };
                if !self.assigned.contains(mname) || self.iterated_maps.contains(mname) {
                    return Ok(false);
                }
                let Some(ValKind::LocalMap(mkk)) = self.var_kinds.get(mname).copied() else {
                    return Ok(false);
                };
                if mkk == MapKeyKind::Unknown {
                    return Ok(false);
                }
                let Some(&mreg) = self.locals.get(mname) else {
                    return Ok(false);
                };
                let ExprKind::Var(vname) = &value.kind else {
                    return Ok(false);
                };
                if vname == mname {
                    return Ok(false);
                }
                let Some(&vreg) = self.locals.get(vname) else {
                    return Ok(false);
                };
                let key_binding = match key {
                    None => None,
                    Some(Expr {
                        kind: ExprKind::Var(kname),
                        ..
                    }) => {
                        if kname == mname || kname == vname {
                            return Ok(false);
                        }
                        let Some(&kreg) = self.locals.get(kname) else {
                            return Ok(false);
                        };
                        Some((kname.clone(), kreg))
                    }
                    Some(_) => return Ok(false),
                };
                if stmts_mutate_var(body, mname) {
                    return Ok(false);
                }
                if !self.record_var_kind(vname, ValKind::Int) {
                    return Ok(false);
                }
                let key_kind = match mkk {
                    MapKeyKind::NonIntAsciiStr => ValKind::AsciiStr {
                        non_int_key: true,
                        non_empty: false,
                    },
                    MapKeyKind::Int => ValKind::Int,
                    MapKeyKind::Unknown => unreachable!(),
                };
                if let Some((ref kname, _)) = key_binding {
                    if !self.record_var_kind(kname, key_kind) {
                        return Ok(false);
                    }
                }

                let saved_outer = self.next_reg;
                let saved_assigned = self.assigned.clone();
                // Snapshot map via Rust native `R.__mapKv(mreg)` -> `[keysArr, valsArr, hasObj]`
                let r_rt = self.alloc()?;
                self.emit(Instr::LoadGlobal {
                    dst: r_rt,
                    idx: self.rt_slot,
                })?;
                let kv = self.alloc()?;
                let mkv_atom = self.string_index("__mapKv");
                self.emit(Instr::CallMethod {
                    dst: kv,
                    obj: r_rt,
                    name: mkv_atom,
                    arg_base: mreg,
                    argc: 1,
                })?;
                let ks = self.alloc()?;
                let vs = self.alloc()?;
                let len = self.alloc()?;
                let idx = self.alloc()?;
                {
                    let t_saved = self.next_reg;
                    let z0 = self.alloc()?;
                    self.emit(Instr::LoadInt { dst: z0, val: 0 })?;
                    self.emit(Instr::GetIndex {
                        dst: ks,
                        obj: kv,
                        key: z0,
                    })?;
                    let z1 = self.alloc()?;
                    self.emit(Instr::LoadInt { dst: z1, val: 1 })?;
                    self.emit(Instr::GetIndex {
                        dst: vs,
                        obj: kv,
                        key: z1,
                    })?;
                    let len_atom = self.string_index("length");
                    self.emit(Instr::GetProp {
                        dst: len,
                        obj: vs,
                        name: len_atom,
                    })?;
                    self.emit(Instr::LoadInt { dst: idx, val: 0 })?;
                    self.next_reg = t_saved;
                }

                let head = self.here();
                let exit = self.emit(Instr::JumpIfNotLt {
                    a: idx,
                    b: len,
                    target: 0,
                })?;
                if let Some((ref kname, kreg)) = key_binding {
                    self.emit(Instr::GetIndex {
                        dst: kreg,
                        obj: ks,
                        key: idx,
                    })?;
                    self.assigned.insert(kname.clone());
                }
                self.emit(Instr::GetIndex {
                    dst: vreg,
                    obj: vs,
                    key: idx,
                })?;
                self.assigned.insert(vname.clone());

                self.loops.push(FastLoop {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                self.iterated_maps.insert(mname.clone());
                let body_ok = self.emit_stmts(body)?;
                self.iterated_maps.remove(mname);
                if !body_ok {
                    return Ok(false);
                }

                let step_ip = self.here();
                let lp = self.loops.pop().unwrap();
                for c in lp.continues {
                    self.patch(c, step_ip);
                }
                self.emit(Instr::AddInt {
                    dst: idx,
                    a: idx,
                    imm: 1,
                    upd: true,
                })?;
                self.emit(Instr::Jump {
                    target: head as u32,
                })?;
                let end = self.here();
                self.patch(exit, end);
                for b in lp.breaks {
                    self.patch(b, end);
                }
                self.assigned = saved_assigned;
                self.next_reg = saved_outer;
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

    /// Resolve a `$this->prop` or `$localObj->prop` property access target to `(obj_reg, prop_atom)`.
    fn resolve_fast_obj_prop(&mut self, target: &Expr, prop: &PropName, nullsafe: bool) -> Option<(Reg, u32)> {
        if nullsafe {
            return None;
        }
        let PropName::Ident(pname) = prop else {
            return None;
        };
        let ExprKind::Var(vname) = &target.kind else {
            return None;
        };
        if vname == "this" {
            let allowed = self.this_int_props.as_ref()?;
            if !allowed.contains(pname) {
                return None;
            }
            let atom = self.string_index(pname);
            return Some((0, atom));
        }
        if !self.assigned.contains(vname) {
            return None;
        }
        if self.var_kinds.get(vname) != Some(&ValKind::LocalObj) {
            return None;
        }
        let &vreg = self.locals.get(vname)?;
        let atom = self.string_index(pname);
        Some((vreg, atom))
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
            ExprKind::String(s) => {
                if self.use_i32_bound || !s.is_ascii() {
                    return Ok(None);
                }
                let dst = self.load_str(s)?;
                Ok(Some((
                    dst,
                    ValKind::AsciiStr {
                        non_int_key: !is_php_canonical_int_str(s),
                        non_empty: !s.is_empty(),
                    },
                )))
            }
            ExprKind::InterpString(parts) => {
                if self.use_i32_bound {
                    return Ok(None);
                }
                if parts.is_empty() {
                    let dst = self.load_str("")?;
                    return Ok(Some((
                        dst,
                        ValKind::AsciiStr {
                            non_int_key: true,
                            non_empty: false,
                        },
                    )));
                }
                let mut acc: Option<Reg> = None;
                let mut acc_is_str = false;
                let mut first_non_empty_seen = false;
                let mut guaranteed_non_int = false;
                let mut non_empty = false;
                for part in parts {
                    match part {
                        StringPart::Literal(s) => {
                            if !s.is_ascii() {
                                return Ok(None);
                            }
                            if s.is_empty() {
                                continue;
                            }
                            if !first_non_empty_seen {
                                first_non_empty_seen = true;
                                guaranteed_non_int = prefix_prevents_php_int_key(s);
                            }
                            non_empty = true;
                            let r = self.load_str(s)?;
                            acc = match acc {
                                None => {
                                    acc_is_str = true;
                                    Some(r)
                                }
                                Some(prev) => {
                                    let dst = self.alloc()?;
                                    self.emit(Instr::Add {
                                        dst,
                                        a: prev,
                                        b: r,
                                    })?;
                                    acc_is_str = true;
                                    Some(dst)
                                }
                            };
                        }
                        StringPart::Expr(e) => {
                            let Some((r, kind)) = self.emit_expr(e)? else {
                                return Ok(None);
                            };
                            match kind {
                                ValKind::Int => {
                                    if !first_non_empty_seen {
                                        first_non_empty_seen = true;
                                    }
                                    non_empty = true;
                                }
                                ValKind::AsciiStr {
                                    non_int_key,
                                    non_empty: ne,
                                } => {
                                    if !first_non_empty_seen && ne {
                                        first_non_empty_seen = true;
                                        guaranteed_non_int = non_int_key;
                                    }
                                    if ne {
                                        non_empty = true;
                                    }
                                }
                                _ => return Ok(None),
                            }
                            acc = match acc {
                                None => {
                                    if matches!(kind, ValKind::AsciiStr { .. }) {
                                        acc_is_str = true;
                                        Some(r)
                                    } else {
                                        let empty = self.load_str("")?;
                                        let dst = self.alloc()?;
                                        self.emit(Instr::Add {
                                            dst,
                                            a: empty,
                                            b: r,
                                        })?;
                                        acc_is_str = true;
                                        Some(dst)
                                    }
                                }
                                Some(prev) => {
                                    let lhs = if acc_is_str || matches!(kind, ValKind::AsciiStr { .. }) {
                                        prev
                                    } else {
                                        let empty = self.load_str("")?;
                                        let s = self.alloc()?;
                                        self.emit(Instr::Add {
                                            dst: s,
                                            a: empty,
                                            b: prev,
                                        })?;
                                        s
                                    };
                                    let dst = self.alloc()?;
                                    self.emit(Instr::Add {
                                        dst,
                                        a: lhs,
                                        b: r,
                                    })?;
                                    acc_is_str = true;
                                    Some(dst)
                                }
                            };
                        }
                    }
                }
                let res = match acc {
                    Some(r) => r,
                    None => self.load_str("")?,
                };
                Ok(Some((
                    res,
                    ValKind::AsciiStr {
                        non_int_key: guaranteed_non_int || !non_empty,
                        non_empty,
                    },
                )))
            }
            ExprKind::Var(name) => {
                if !self.assigned.contains(name) {
                    return Ok(None);
                }
                let Some(&reg) = self.locals.get(name) else {
                    return Ok(None);
                };
                let Some(&kind) = self.var_kinds.get(name) else {
                    return Ok(None);
                };
                // Never allow `LocalMap` or `LocalObj` to be loaded as a generic value
                // (prevents aliasing/escaping).
                if matches!(kind, ValKind::LocalMap(_) | ValKind::LocalObj) {
                    return Ok(None);
                }
                Ok(Some((reg, kind)))
            }
            ExprKind::Prop {
                target,
                prop,
                nullsafe,
            } => {
                if self.use_i32_bound {
                    return Ok(None);
                }
                let Some((obj_reg, atom)) = self.resolve_fast_obj_prop(target, prop, *nullsafe) else {
                    return Ok(None);
                };
                let dst = self.alloc()?;
                self.emit(Instr::GetProp {
                    dst,
                    obj: obj_reg,
                    name: atom,
                })?;
                Ok(Some((dst, ValKind::Int)))
            }
            ExprKind::Index {
                target,
                index: Some(kexpr),
            } => {
                if self.use_i32_bound {
                    return Ok(None);
                }
                let ExprKind::Var(mname) = &target.kind else {
                    return Ok(None);
                };
                if !self.assigned.contains(mname) {
                    return Ok(None);
                }
                let Some(ValKind::LocalMap(mkk)) = self.var_kinds.get(mname).copied() else {
                    return Ok(None);
                };
                if mkk == MapKeyKind::Unknown {
                    return Ok(None);
                }
                let Some(&mreg) = self.locals.get(mname) else {
                    return Ok(None);
                };
                let Some((kr, kk)) = self.emit_expr(kexpr)? else {
                    return Ok(None);
                };
                let ok_key = match (mkk, kk) {
                    (MapKeyKind::NonIntAsciiStr, ValKind::AsciiStr { non_int_key: true, .. }) => true,
                    (MapKeyKind::Int, ValKind::Int) => true,
                    _ => false,
                };
                if !ok_key {
                    return Ok(None);
                }
                let dst = self.alloc()?;
                let get_atom = self.string_index("get");
                let callee = if let Some((_, get_reg)) = self.map_method_regs {
                    get_reg
                } else {
                    let c = self.alloc()?;
                    self.emit(Instr::GetProp {
                        dst: c,
                        obj: mreg,
                        name: get_atom,
                    })?;
                    c
                };
                self.emit(Instr::CallWithThis {
                    dst,
                    callee,
                    this_v: mreg,
                    arg_base: kr,
                    argc: 1,
                    name: get_atom,
                })?;
                // Catches `undefined` on missing key -> transparent fallback to exact PHP Warning + null!
                self.emit_range_guard(dst)?;
                Ok(Some((dst, ValKind::Int)))
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
                    let imm = match op {
                        UnaryOp::PreInc | UnaryOp::PostInc => 1i32,
                        _ => -1i32,
                    };
                    let is_post = matches!(op, UnaryOp::PostInc | UnaryOp::PostDec);
                    if let ExprKind::Prop {
                        target,
                        prop,
                        nullsafe,
                    } = &inner.kind
                    {
                        if self.use_i32_bound {
                            return Ok(None);
                        }
                        let Some((obj_reg, atom)) =
                            self.resolve_fast_obj_prop(target, prop, *nullsafe)
                        else {
                            return Ok(None);
                        };
                        let cur = self.alloc()?;
                        self.emit(Instr::GetProp {
                            dst: cur,
                            obj: obj_reg,
                            name: atom,
                        })?;
                        let next_v = self.alloc()?;
                        self.emit(Instr::AddInt {
                            dst: next_v,
                            a: cur,
                            imm,
                            upd: false,
                        })?;
                        self.emit_range_guard(next_v)?;
                        self.emit(Instr::SetProp {
                            obj: obj_reg,
                            name: atom,
                            val: next_v,
                            strict: false,
                        })?;
                        return Ok(Some((if is_post { cur } else { next_v }, ValKind::Int)));
                    }
                    let ExprKind::Var(name) = &inner.kind else {
                        return Ok(None);
                    };
                    if !self.assigned.contains(name)
                        || self.var_kinds.get(name) != Some(&ValKind::Int)
                    {
                        return Ok(None);
                    }
                    let Some(&vreg) = self.locals.get(name) else {
                        return Ok(None);
                    };
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
                // Case 1: `$map[$k] = $v`
                if let ExprKind::Index {
                    target: arr_t,
                    index: Some(kexpr),
                } = &target.kind
                {
                    if *op != AssignOp::Assign || self.use_i32_bound {
                        return Ok(None);
                    }
                    let ExprKind::Var(mname) = &arr_t.kind else {
                        return Ok(None);
                    };
                    if !self.assigned.contains(mname) || self.iterated_maps.contains(mname) {
                        return Ok(None);
                    }
                    let Some(ValKind::LocalMap(mkk)) = self.var_kinds.get(mname).copied() else {
                        return Ok(None);
                    };
                    let Some(&mreg) = self.locals.get(mname) else {
                        return Ok(None);
                    };
                    let Some((kr, kk)) = self.emit_expr(kexpr)? else {
                        return Ok(None);
                    };
                    let new_mkk = match (mkk, kk) {
                        (
                            MapKeyKind::Unknown | MapKeyKind::NonIntAsciiStr,
                            ValKind::AsciiStr {
                                non_int_key: true, ..
                            },
                        ) => MapKeyKind::NonIntAsciiStr,
                        (MapKeyKind::Unknown | MapKeyKind::Int, ValKind::Int) => MapKeyKind::Int,
                        _ => return Ok(None),
                    };
                    self.var_kinds
                        .insert(mname.clone(), ValKind::LocalMap(new_mkk));
                    let Some((vr, ValKind::Int)) = self.emit_expr(value)? else {
                        return Ok(None);
                    };
                    let set_atom = self.string_index("set");
                    let callee = if let Some((set_reg, _)) = self.map_method_regs {
                        set_reg
                    } else {
                        let c = self.alloc()?;
                        self.emit(Instr::GetProp {
                            dst: c,
                            obj: mreg,
                            name: set_atom,
                        })?;
                        c
                    };
                    let arg_base = self.alloc_block(2)?;
                    self.emit(Instr::Move {
                        dst: arg_base,
                        src: kr,
                    })?;
                    self.emit(Instr::Move {
                        dst: arg_base + 1,
                        src: vr,
                    })?;
                    let tmp = self.alloc()?;
                    self.emit(Instr::CallWithThis {
                        dst: tmp,
                        callee,
                        this_v: mreg,
                        arg_base,
                        argc: 2,
                        name: set_atom,
                    })?;
                    return Ok(Some((vr, ValKind::Int)));
                }

                // Case 2: `$this->prop = / += / -= / *= expr` or `$localObj->prop = / += ...`
                if let ExprKind::Prop {
                    target: pt,
                    prop,
                    nullsafe,
                } = &target.kind
                {
                    if self.use_i32_bound {
                        return Ok(None);
                    }
                    let Some((obj_reg, atom)) = self.resolve_fast_obj_prop(pt, prop, *nullsafe)
                    else {
                        return Ok(None);
                    };
                    match op {
                        AssignOp::Assign => {
                            let Some((vr, ValKind::Int)) = self.emit_expr(value)? else {
                                return Ok(None);
                            };
                            self.emit(Instr::SetProp {
                                obj: obj_reg,
                                name: atom,
                                val: vr,
                                strict: false,
                            })?;
                            return Ok(Some((vr, ValKind::Int)));
                        }
                        AssignOp::Add | AssignOp::Sub | AssignOp::Mul => {
                            let cur = self.alloc()?;
                            self.emit(Instr::GetProp {
                                dst: cur,
                                obj: obj_reg,
                                name: atom,
                            })?;
                            let Some((vr, ValKind::Int)) = self.emit_expr(value)? else {
                                return Ok(None);
                            };
                            let dst = self.alloc()?;
                            match op {
                                AssignOp::Add => self.emit(Instr::Add { dst, a: cur, b: vr })?,
                                AssignOp::Sub => self.emit(Instr::Sub { dst, a: cur, b: vr })?,
                                AssignOp::Mul => self.emit(Instr::Mul { dst, a: cur, b: vr })?,
                                _ => unreachable!(),
                            };
                            self.emit_range_guard(dst)?;
                            self.emit(Instr::SetProp {
                                obj: obj_reg,
                                name: atom,
                                val: dst,
                                strict: false,
                            })?;
                            return Ok(Some((dst, ValKind::Int)));
                        }
                        _ => return Ok(None),
                    }
                }

                // Case 3: local variable assignment
                let ExprKind::Var(name) = &target.kind else {
                    return Ok(None);
                };
                let Some(&vreg) = self.locals.get(name) else {
                    return Ok(None);
                };
                match op {
                    AssignOp::Assign => {
                        // `$map = []`
                        if let ExprKind::Array(items) = &value.kind {
                            if !items.is_empty()
                                || self.use_i32_bound
                                || self.iterated_maps.contains(name)
                            {
                                return Ok(None);
                            }
                            if !self.record_var_kind(name, ValKind::LocalMap(MapKeyKind::Unknown)) {
                                return Ok(None);
                            }
                            let saved = self.next_reg;
                            let r_rt = self.alloc()?;
                            self.emit(Instr::LoadGlobal {
                                dst: r_rt,
                                idx: self.rt_slot,
                            })?;
                            let nm_atom = self.string_index("__newMap");
                            self.emit(Instr::CallMethod {
                                dst: vreg,
                                obj: r_rt,
                                name: nm_atom,
                                arg_base: vreg,
                                argc: 0,
                            })?;
                            if let Some((set_reg, get_reg)) = self.map_method_regs {
                                let set_atom = self.string_index("set");
                                self.emit(Instr::GetProp {
                                    dst: set_reg,
                                    obj: vreg,
                                    name: set_atom,
                                })?;
                                let get_atom = self.string_index("get");
                                self.emit(Instr::GetProp {
                                    dst: get_reg,
                                    obj: vreg,
                                    name: get_atom,
                                })?;
                            }
                            self.next_reg = saved;
                            self.assigned.insert(name.clone());
                            return Ok(Some((vreg, ValKind::LocalMap(MapKeyKind::Unknown))));
                        }

                        // `$c = new ClassName(int_args...)` (non-escaping LocalObj!)
                        if let ExprKind::New { class, args } = &value.kind {
                            if self.use_i32_bound || !self.local_obj_uses.contains_key(name) {
                                return Ok(None);
                            }
                            let ClassRef::Name(cname) = class else {
                                return Ok(None);
                            };
                            if args.len() > 2 || args.iter().any(|a| a.unpack || a.name.is_some()) {
                                return Ok(None);
                            }
                            let mut eval_args = Vec::with_capacity(args.len());
                            for a in args {
                                let Some((ar, ValKind::Int)) = self.emit_expr(&a.value)? else {
                                    return Ok(None);
                                };
                                eval_args.push(ar);
                            }
                            if !self.record_var_kind(name, ValKind::LocalObj) {
                                return Ok(None);
                            }
                            let fqn_lo = self
                                .ns
                                .resolve_class(cname)
                                .trim_start_matches('\\')
                                .to_ascii_lowercase();
                            let props_used: Vec<String> = self
                                .local_obj_uses
                                .get(name)
                                .map(|u| u.props.iter().cloned().collect())
                                .unwrap_or_default();
                            let methods_used: Vec<(String, usize, bool)> = self
                                .local_obj_uses
                                .get(name)
                                .map(|u| {
                                    u.methods
                                        .iter()
                                        .map(|((m, a), r)| (m.clone(), *a, *r))
                                        .collect()
                                })
                                .unwrap_or_default();

                            let saved = self.next_reg;
                            let r_rt = self.alloc()?;
                            self.emit(Instr::LoadGlobal {
                                dst: r_rt,
                                idx: self.rt_slot,
                            })?;
                            let cls_map = self.alloc()?;
                            let cm_atom = self.string_index("__clsMap");
                            self.emit(Instr::GetProp {
                                dst: cls_map,
                                obj: r_rt,
                                name: cm_atom,
                            })?;
                            let cls_key = self.load_str(&fqn_lo)?;
                            let cls = self.alloc()?;
                            let get_atom = self.string_index("get");
                            self.emit(Instr::CallMethod {
                                dst: cls,
                                obj: cls_map,
                                name: get_atom,
                                arg_base: cls_key,
                                argc: 1,
                            })?;
                            let j_no_cls = self.emit(Instr::JumpIfFalse {
                                cond: cls,
                                target: 0,
                            })?;
                            self.bails.push(j_no_cls);

                            let ctor = self.alloc()?;
                            let ctor_atom = self.string_index("fastNumCtor");
                            self.emit(Instr::GetProp {
                                dst: ctor,
                                obj: cls,
                                name: ctor_atom,
                            })?;
                            let j_no_ctor = self.emit(Instr::JumpIfFalse {
                                cond: ctor,
                                target: 0,
                            })?;
                            self.bails.push(j_no_ctor);

                            let min_a = self.alloc()?;
                            let min_atom = self.string_index("fastNumMinArgs");
                            self.emit(Instr::GetProp {
                                dst: min_a,
                                obj: cls,
                                name: min_atom,
                            })?;
                            let max_a = self.alloc()?;
                            let max_atom = self.string_index("fastNumMaxArgs");
                            self.emit(Instr::GetProp {
                                dst: max_a,
                                obj: cls,
                                name: max_atom,
                            })?;
                            let argc_r = self.alloc()?;
                            self.emit(Instr::LoadInt {
                                dst: argc_r,
                                val: eval_args.len() as i32,
                            })?;
                            let j_min = self.emit(Instr::JumpIfNotLe {
                                a: min_a,
                                b: argc_r,
                                target: 0,
                            })?;
                            self.bails.push(j_min);
                            let j_max = self.emit(Instr::JumpIfNotLe {
                                a: argc_r,
                                b: max_a,
                                target: 0,
                            })?;
                            self.bails.push(j_max);

                            if !props_used.is_empty() {
                                let fps = self.alloc()?;
                                let fps_atom = self.string_index("fastNumProps");
                                self.emit(Instr::GetProp {
                                    dst: fps,
                                    obj: cls,
                                    name: fps_atom,
                                })?;
                                let j_no_fps = self.emit(Instr::JumpIfFalse {
                                    cond: fps,
                                    target: 0,
                                })?;
                                self.bails.push(j_no_fps);
                                let hp = self.alloc()?;
                                for p in &props_used {
                                    let pa = self.string_index(p);
                                    self.emit(Instr::GetProp {
                                        dst: hp,
                                        obj: fps,
                                        name: pa,
                                    })?;
                                    let j_p = self.emit(Instr::JumpIfFalse {
                                        cond: hp,
                                        target: 0,
                                    })?;
                                    self.bails.push(j_p);
                                }
                            }

                            if !methods_used.is_empty() {
                                let mobj = self.alloc()?;
                                let mobj_atom = self.string_index("mObj");
                                self.emit(Instr::GetProp {
                                    dst: mobj,
                                    obj: cls,
                                    name: mobj_atom,
                                })?;
                                let j_no_mobj = self.emit(Instr::JumpIfFalse {
                                    cond: mobj,
                                    target: 0,
                                })?;
                                self.bails.push(j_no_mobj);
                                let mm = self.alloc()?;
                                let ar = self.alloc()?;
                                let exp = self.alloc()?;
                                let eq = self.alloc()?;
                                let ar_atom = self.string_index("fnumArity");
                                let ri_atom = self.string_index("fnumRetInt");
                                for (mlo, marity, needs_ret_int) in &methods_used {
                                    let ma = self.string_index(mlo);
                                    self.emit(Instr::GetProp {
                                        dst: mm,
                                        obj: mobj,
                                        name: ma,
                                    })?;
                                    let j_m = self.emit(Instr::JumpIfFalse {
                                        cond: mm,
                                        target: 0,
                                    })?;
                                    self.bails.push(j_m);
                                    self.emit(Instr::GetProp {
                                        dst: ar,
                                        obj: mm,
                                        name: ar_atom,
                                    })?;
                                    self.emit(Instr::LoadInt {
                                        dst: exp,
                                        val: *marity as i32,
                                    })?;
                                    self.emit(Instr::Eq {
                                        dst: eq,
                                        a: ar,
                                        b: exp,
                                    })?;
                                    let j_ar = self.emit(Instr::JumpIfFalse {
                                        cond: eq,
                                        target: 0,
                                    })?;
                                    self.bails.push(j_ar);
                                    if *needs_ret_int {
                                        self.emit(Instr::GetProp {
                                            dst: eq,
                                            obj: mm,
                                            name: ri_atom,
                                        })?;
                                        let j_ri = self.emit(Instr::JumpIfFalse {
                                            cond: eq,
                                            target: 0,
                                        })?;
                                        self.bails.push(j_ri);
                                    }
                                }
                            }

                            let argc = eval_args.len() as u16;
                            let arg_base = if argc == 0 {
                                vreg
                            } else if argc == 1 {
                                eval_args[0]
                            } else {
                                let base = self.alloc_block(argc)?;
                                for (i, ar) in eval_args.into_iter().enumerate() {
                                    self.emit(Instr::Move {
                                        dst: base + i as u16,
                                        src: ar,
                                    })?;
                                }
                                base
                            };
                            self.emit(Instr::Call {
                                dst: vreg,
                                callee: ctor,
                                arg_base,
                                argc,
                            })?;
                            self.next_reg = saved;
                            self.assigned.insert(name.clone());
                            return Ok(Some((vreg, ValKind::LocalObj)));
                        }

                        let Some((vr, vk)) = self.emit_expr(value)? else {
                            return Ok(None);
                        };
                        if !matches!(vk, ValKind::Int | ValKind::AsciiStr { .. }) {
                            return Ok(None);
                        }
                        if !self.record_var_kind(name, vk) {
                            return Ok(None);
                        }
                        if vr != vreg {
                            self.emit(Instr::Move { dst: vreg, src: vr })?;
                        }
                        self.assigned.insert(name.clone());
                        Ok(Some((vreg, vk)))
                    }
                    AssignOp::Add | AssignOp::Sub | AssignOp::Mul => {
                        if !self.assigned.contains(name)
                            || self.var_kinds.get(name) != Some(&ValKind::Int)
                        {
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
                BinOp::Concat => {
                    if self.use_i32_bound {
                        return Ok(None);
                    }
                    let Some((a, ka)) = self.emit_expr(left)? else {
                        return Ok(None);
                    };
                    let Some((b, kb)) = self.emit_expr(right)? else {
                        return Ok(None);
                    };
                    if !matches!(ka, ValKind::Int | ValKind::AsciiStr { .. })
                        || !matches!(kb, ValKind::Int | ValKind::AsciiStr { .. })
                    {
                        return Ok(None);
                    }
                    let lhs = if matches!(ka, ValKind::AsciiStr { .. })
                        || matches!(kb, ValKind::AsciiStr { .. })
                    {
                        a
                    } else {
                        let empty = self.load_str("")?;
                        let s = self.alloc()?;
                        self.emit(Instr::Add {
                            dst: s,
                            a: empty,
                            b: a,
                        })?;
                        s
                    };
                    let dst = self.alloc()?;
                    self.emit(Instr::Add { dst, a: lhs, b })?;
                    let non_int_key = match &left.kind {
                        ExprKind::String(s) if !s.is_empty() => prefix_prevents_php_int_key(s),
                        _ => matches!(
                            ka,
                            ValKind::AsciiStr {
                                non_int_key: true,
                                non_empty: true
                            }
                        ),
                    };
                    let non_empty = matches!(ka, ValKind::Int | ValKind::AsciiStr { non_empty: true, .. })
                        || matches!(kb, ValKind::Int | ValKind::AsciiStr { non_empty: true, .. });
                    Ok(Some((
                        dst,
                        ValKind::AsciiStr {
                            non_int_key,
                            non_empty,
                        },
                    )))
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    let left_strictly_bounded = matches!(
                        &left.kind,
                        ExprKind::Var(_) | ExprKind::Binary { op: BinOp::Mul, .. }
                    );
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
                                if !((-16..=16).contains(&imm)
                                    && (self.use_i32_bound || left_strictly_bounded))
                                {
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
                                if !((-16..=16).contains(&imm)
                                    && (self.use_i32_bound || left_strictly_bounded))
                                {
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
            ExprKind::MethodCall {
                target,
                method,
                nullsafe,
                args: CallArgs::Normal(list),
            } => {
                if self.use_i32_bound || *nullsafe || list.len() > 2 {
                    return Ok(None);
                }
                if list.iter().any(|a| a.unpack || a.name.is_some()) {
                    return Ok(None);
                }
                let ExprKind::Var(vname) = &target.kind else {
                    return Ok(None);
                };
                if !self.assigned.contains(vname)
                    || self.var_kinds.get(vname) != Some(&ValKind::LocalObj)
                {
                    return Ok(None);
                }
                let PropName::Ident(mname) = method else {
                    return Ok(None);
                };
                let Some(&vreg) = self.locals.get(vname) else {
                    return Ok(None);
                };
                let argc = list.len() as u16;
                let arg_base = if argc == 0 {
                    vreg
                } else if argc == 1 {
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
                let mkey = format!("\x01{}", mname.to_ascii_lowercase());
                let atom = self.string_index(&mkey);
                self.emit(Instr::CallMethod {
                    dst,
                    obj: vreg,
                    name: atom,
                    arg_base,
                    argc,
                })?;
                // Catches `undefined` if callee bailed on overflow!
                self.emit_range_guard(dst)?;
                Ok(Some((dst, ValKind::Int)))
            }
            ExprKind::Call {
                callee: Callee::Name(n),
                args: CallArgs::Normal(list),
            } => {
                if list.iter().any(|a| a.unpack || a.name.is_some()) {
                    return Ok(None);
                }
                // `strlen($asciiStr)` -> direct `s.length`!
                if !self.use_i32_bound && list.len() == 1 && self.is_global_builtin(n, "strlen") {
                    let Some((sr, ValKind::AsciiStr { .. })) = self.emit_expr(&list[0].value)?
                    else {
                        return Ok(None);
                    };
                    let dst = self.alloc()?;
                    let len_atom = self.string_index("length");
                    self.emit(Instr::GetProp {
                        dst,
                        obj: sr,
                        name: len_atom,
                    })?;
                    return Ok(Some((dst, ValKind::Int)));
                }
                // `count($localMap)` / `sizeof($localMap)` -> direct `m.size`!
                if !self.use_i32_bound
                    && list.len() == 1
                    && (self.is_global_builtin(n, "count") || self.is_global_builtin(n, "sizeof"))
                {
                    if let ExprKind::Var(mname) = &list[0].value.kind {
                        if self.assigned.contains(mname)
                            && matches!(self.var_kinds.get(mname), Some(ValKind::LocalMap(_)))
                        {
                            let &mreg = self.locals.get(mname).unwrap();
                            let dst = self.alloc()?;
                            let sz_atom = self.string_index("size");
                            self.emit(Instr::GetProp {
                                dst,
                                obj: mreg,
                                name: sz_atom,
                            })?;
                            return Ok(Some((dst, ValKind::Int)));
                        }
                    }
                    return Ok(None);
                }

                // Direct self-recursive call (Tier-A `fn_int`)
                let Some(slot) = self.self_global_slot else {
                    return Ok(None);
                };
                if !self.is_self_name(n) || list.len() != self.param_count as usize {
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

// ---- AST helpers for soundness checks ----

fn stmts_mutate_var(stmts: &[Stmt], name: &str) -> bool {
    for s in stmts {
        match &s.kind {
            StmtKind::Block(b) => {
                if stmts_mutate_var(b, name) {
                    return true;
                }
            }
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => {
                if expr_mutates_var(e, name) {
                    return true;
                }
            }
            StmtKind::If {
                cond,
                then_body,
                elseifs,
                else_body,
            } => {
                if expr_mutates_var(cond, name)
                    || stmts_mutate_var(then_body, name)
                    || elseifs
                        .iter()
                        .any(|(c, b)| expr_mutates_var(c, name) || stmts_mutate_var(b, name))
                    || else_body
                        .as_deref()
                        .is_some_and(|b| stmts_mutate_var(b, name))
                {
                    return true;
                }
            }
            StmtKind::While { cond, body } | StmtKind::DoWhile { body, cond } => {
                if expr_mutates_var(cond, name) || stmts_mutate_var(body, name) {
                    return true;
                }
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                if init.iter().any(|e| expr_mutates_var(e, name))
                    || cond.iter().any(|e| expr_mutates_var(e, name))
                    || step.iter().any(|e| expr_mutates_var(e, name))
                    || stmts_mutate_var(body, name)
                {
                    return true;
                }
            }
            StmtKind::ForEach {
                expr,
                key,
                value,
                body,
                ..
            } => {
                if expr_mutates_var(expr, name)
                    || matches!(key, Some(Expr { kind: ExprKind::Var(k), .. }) if k == name)
                    || matches!(value, Expr { kind: ExprKind::Var(v), .. } if v == name)
                    || stmts_mutate_var(body, name)
                {
                    return true;
                }
            }
            StmtKind::Unset(list) => {
                if list.iter().any(|e| target_root_var(e) == Some(name)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn target_root_var(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Var(n) => Some(n.as_str()),
        ExprKind::Index { target, .. } | ExprKind::Prop { target, .. } => target_root_var(target),
        _ => None,
    }
}

fn expr_mutates_var(expr: &Expr, name: &str) -> bool {
    match &expr.kind {
        ExprKind::Assign { target, value, .. } => {
            target_root_var(target) == Some(name) || expr_mutates_var(value, name)
        }
        ExprKind::Binary { left, right, .. } => {
            expr_mutates_var(left, name) || expr_mutates_var(right, name)
        }
        ExprKind::Unary { op, expr: inner } => {
            if matches!(
                op,
                UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec
            ) && target_root_var(inner) == Some(name)
            {
                return true;
            }
            expr_mutates_var(inner, name)
        }
        ExprKind::Call {
            args: CallArgs::Normal(list),
            ..
        }
        | ExprKind::MethodCall {
            args: CallArgs::Normal(list),
            ..
        } => list.iter().any(|a| expr_mutates_var(&a.value, name)),
        ExprKind::InterpString(parts) => parts.iter().any(|p| match p {
            StringPart::Expr(e) => expr_mutates_var(e, name),
            _ => false,
        }),
        _ => false,
    }
}

/// Pre-scan a function body to discover non-escaping local object variables (`$c = new C(...)`)
/// and record every method call & property access performed on them.
fn collect_local_obj_uses(
    stmts: &[Stmt],
    param_names: &HashSet<String>,
) -> HashMap<String, LocalObjUsage> {
    let mut new_counts: HashMap<String, usize> = HashMap::new();
    let mut disqualified: HashSet<String> = param_names.clone();
    let mut uses: HashMap<String, LocalObjUsage> = HashMap::new();

    fn scan_stmts(
        stmts: &[Stmt],
        new_counts: &mut HashMap<String, usize>,
        disqualified: &mut HashSet<String>,
        uses: &mut HashMap<String, LocalObjUsage>,
    ) {
        for s in stmts {
            match &s.kind {
                StmtKind::Block(b) => scan_stmts(b, new_counts, disqualified, uses),
                StmtKind::Expr(e) => scan_expr(e, false, new_counts, disqualified, uses),
                StmtKind::Return(Some(e)) => scan_expr(e, true, new_counts, disqualified, uses),
                StmtKind::If {
                    cond,
                    then_body,
                    elseifs,
                    else_body,
                } => {
                    scan_expr(cond, true, new_counts, disqualified, uses);
                    scan_stmts(then_body, new_counts, disqualified, uses);
                    for (c, b) in elseifs {
                        scan_expr(c, true, new_counts, disqualified, uses);
                        scan_stmts(b, new_counts, disqualified, uses);
                    }
                    if let Some(eb) = else_body {
                        scan_stmts(eb, new_counts, disqualified, uses);
                    }
                }
                StmtKind::While { cond, body } | StmtKind::DoWhile { body, cond } => {
                    scan_expr(cond, true, new_counts, disqualified, uses);
                    scan_stmts(body, new_counts, disqualified, uses);
                }
                StmtKind::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    for e in init {
                        scan_expr(e, false, new_counts, disqualified, uses);
                    }
                    for e in cond {
                        scan_expr(e, true, new_counts, disqualified, uses);
                    }
                    for e in step {
                        scan_expr(e, false, new_counts, disqualified, uses);
                    }
                    scan_stmts(body, new_counts, disqualified, uses);
                }
                StmtKind::ForEach {
                    expr,
                    key,
                    value,
                    body,
                    ..
                } => {
                    scan_expr(expr, true, new_counts, disqualified, uses);
                    if let Some(Expr {
                        kind: ExprKind::Var(k),
                        ..
                    }) = key
                    {
                        disqualified.insert(k.clone());
                    }
                    if let ExprKind::Var(v) = &value.kind {
                        disqualified.insert(v.clone());
                    }
                    scan_stmts(body, new_counts, disqualified, uses);
                }
                _ => {}
            }
        }
    }

    fn scan_expr(
        expr: &Expr,
        result_used: bool,
        new_counts: &mut HashMap<String, usize>,
        disqualified: &mut HashSet<String>,
        uses: &mut HashMap<String, LocalObjUsage>,
    ) {
        match &expr.kind {
            ExprKind::Var(n) => {
                // Bare variable occurrence (not receiver of `->`) disqualifies it from LocalObj.
                disqualified.insert(n.clone());
            }
            ExprKind::Assign { op, target, value } => {
                if *op == AssignOp::Assign {
                    if let (ExprKind::Var(vname), ExprKind::New { args, .. }) =
                        (&target.kind, &value.kind)
                    {
                        *new_counts.entry(vname.clone()).or_insert(0) += 1;
                        if result_used {
                            disqualified.insert(vname.clone());
                        }
                        for a in args {
                            scan_expr(&a.value, true, new_counts, disqualified, uses);
                        }
                        return;
                    }
                }
                if let ExprKind::Prop {
                    target: pt,
                    prop: PropName::Ident(pname),
                    nullsafe: false,
                } = &target.kind
                {
                    if let ExprKind::Var(vname) = &pt.kind {
                        uses.entry(vname.clone())
                            .or_default()
                            .props
                            .insert(pname.clone());
                    } else {
                        scan_expr(target, true, new_counts, disqualified, uses);
                    }
                } else {
                    scan_expr(target, true, new_counts, disqualified, uses);
                }
                scan_expr(value, true, new_counts, disqualified, uses);
            }
            ExprKind::Prop {
                target,
                prop,
                nullsafe,
            } => {
                if !*nullsafe {
                    if let (ExprKind::Var(vname), PropName::Ident(pname)) = (&target.kind, prop) {
                        uses.entry(vname.clone())
                            .or_default()
                            .props
                            .insert(pname.clone());
                        return;
                    }
                }
                scan_expr(target, true, new_counts, disqualified, uses);
            }
            ExprKind::MethodCall {
                target,
                method,
                nullsafe,
                args: CallArgs::Normal(list),
            } => {
                if !*nullsafe {
                    if let (ExprKind::Var(vname), PropName::Ident(mname)) = (&target.kind, method) {
                        let entry = uses
                            .entry(vname.clone())
                            .or_default()
                            .methods
                            .entry((mname.to_ascii_lowercase(), list.len()))
                            .or_insert(false);
                        if result_used {
                            *entry = true;
                        }
                        for a in list {
                            scan_expr(&a.value, true, new_counts, disqualified, uses);
                        }
                        return;
                    }
                }
                scan_expr(target, true, new_counts, disqualified, uses);
                for a in list {
                    scan_expr(&a.value, true, new_counts, disqualified, uses);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                scan_expr(left, true, new_counts, disqualified, uses);
                scan_expr(right, true, new_counts, disqualified, uses);
            }
            ExprKind::Unary { expr: inner, .. } => {
                scan_expr(inner, true, new_counts, disqualified, uses);
            }
            ExprKind::Index { target, index } => {
                scan_expr(target, true, new_counts, disqualified, uses);
                if let Some(i) = index {
                    scan_expr(i, true, new_counts, disqualified, uses);
                }
            }
            ExprKind::InterpString(parts) => {
                for p in parts {
                    if let StringPart::Expr(e) = p {
                        scan_expr(e, true, new_counts, disqualified, uses);
                    }
                }
            }
            ExprKind::Call {
                args: CallArgs::Normal(list),
                ..
            } => {
                for a in list {
                    scan_expr(&a.value, true, new_counts, disqualified, uses);
                }
            }
            _ => {}
        }
    }

    scan_stmts(stmts, &mut new_counts, &mut disqualified, &mut uses);
    let mut out = HashMap::new();
    for (v, cnt) in new_counts {
        if cnt == 1 && !disqualified.contains(&v) {
            out.insert(v.clone(), uses.remove(&v).unwrap_or_default());
        }
    }
    out
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

fn ret_type_is_void(ty: &Option<TypeHint>) -> bool {
    matches!(
        ty,
        Some(TypeHint::Simple(n)) if n.parts.len() == 1 && n.parts[0].eq_ignore_ascii_case("void")
    )
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

/// Attempt to compile a pure-JS native-JIT companion for a PHP function.
/// Returns `Ok(None)` if the function falls outside the sound native-JIT subset.
pub(super) fn try_compile_fast_int(
    fqn: &str,
    ns: &NsResolver,
    rt_slot: u32,
    decl: &FunctionDecl,
    info: &FnScopeInfo,
    sink: &FuncSink<'_>,
) -> R<Option<FastIntCompiled>> {
    if decl.by_ref
        || info.use_scope_map
        || info.is_generator
        || info.uses_func_args
        || decl.params.is_empty()
        || decl.params.len() > 2
        || !ret_type_allows_int(&decl.return_type)
    {
        return Ok(None);
    }
    let mut param_names = HashSet::new();
    for p in &decl.params {
        if p.by_ref
            || p.variadic
            || p.default.is_some()
            || is_superglobal(&p.name)
            || !param_type_allows_int(&p.ty)
        {
            return Ok(None);
        }
        param_names.insert(p.name.clone());
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
    let mut var_kinds = HashMap::new();
    let mut assigned = HashSet::new();
    // JS calling convention: reg 0 = `this`, reg 1..=param_count = params.
    let mut next_reg: Reg = 1;
    for p in &decl.params {
        locals.insert(p.name.clone(), next_reg);
        var_kinds.insert(p.name.clone(), ValKind::Int);
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
    let local_obj_uses = if use_i32_bound {
        HashMap::new()
    } else {
        collect_local_obj_uses(&decl.body, &param_names)
    };

    let mut builder = FastIntBuilder {
        name: format!("__php_fi_{}", fqn),
        fqn_lo,
        ns: ns.clone(),
        rt_slot,
        use_i32_bound,
        self_global_slot,
        param_count: decl.params.len() as u16,
        this_int_props: None,
        allow_void_return: false,
        code: Vec::new(),
        constants: Vec::new(),
        string_constants: Vec::new(),
        string_ids: HashMap::new(),
        string_consts: HashMap::new(),
        float_consts: HashMap::new(),
        next_reg,
        max_reg: next_reg,
        locals,
        var_kinds,
        assigned,
        local_obj_uses,
        iterated_maps: HashSet::new(),
        map_method_regs: None,
        loops: Vec::new(),
        bails: Vec::new(),
        has_self_call: false,
        bound_regs: None,
    };
    if !builder.use_i32_bound {
        let r_lo = builder.load_f64(-(TIER_C_BOUND as f64))?;
        let r_hi = builder.load_f64(TIER_C_BOUND as f64)?;
        builder.bound_regs = Some((r_lo, r_hi));
        let r_mset = builder.alloc()?;
        let r_mget = builder.alloc()?;
        builder.map_method_regs = Some((r_mset, r_mget));
    }

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
    proto.string_constants = builder.string_constants;
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

/// Attempt to compile a pure-int Number-domain method companion (`__php_fi_m_*`)
/// for a public non-static PHP class method accessing only `$this->fastIntProp`
/// and integer operations. Used by `LocalObj` call sites (`c.\x01method(arg)`).
#[allow(clippy::too_many_arguments)]
pub(super) fn try_compile_fast_method(
    class_fqn: &str,
    method_name: &str,
    ns: &NsResolver,
    rt_slot: u32,
    params: &[Param],
    return_type: &Option<TypeHint>,
    body: &[Stmt],
    info: &FnScopeInfo,
    fast_int_props: &HashSet<String>,
    sink: &FuncSink<'_>,
) -> R<Option<FastMethodCompiled>> {
    let is_void = ret_type_is_void(return_type);
    let returns_int = !is_void && ret_type_allows_int(return_type);
    if (!returns_int && !is_void)
        || info.use_scope_map
        || info.is_generator
        || info.uses_func_args
        || params.len() > 2
        || fast_int_props.is_empty()
    {
        return Ok(None);
    }
    for p in params {
        if p.by_ref
            || p.variadic
            || p.default.is_some()
            || is_superglobal(&p.name)
            || !param_type_allows_int(&p.ty)
        {
            return Ok(None);
        }
    }

    let mut locals = HashMap::new();
    let mut var_kinds = HashMap::new();
    let mut assigned = HashSet::new();
    // JS calling convention: reg 0 = `this` (plain JS object `{ prop: Number, ... }`), reg 1..=param_count = params.
    let mut next_reg: Reg = 1;
    for p in params {
        locals.insert(p.name.clone(), next_reg);
        var_kinds.insert(p.name.clone(), ValKind::Int);
        assigned.insert(p.name.clone());
        next_reg += 1;
    }
    for vname in &info.locals {
        if !locals.contains_key(vname) && !is_superglobal(vname) && vname != "this" {
            locals.insert(vname.clone(), next_reg);
            next_reg += 1;
        }
    }

    let mut builder = FastIntBuilder {
        name: format!("__php_fi_m_{}_{}", class_fqn, method_name),
        fqn_lo: String::new(),
        ns: ns.clone(),
        rt_slot,
        use_i32_bound: false,
        self_global_slot: None,
        param_count: params.len() as u16,
        this_int_props: Some(fast_int_props.clone()),
        allow_void_return: is_void,
        code: Vec::new(),
        constants: Vec::new(),
        string_constants: Vec::new(),
        string_ids: HashMap::new(),
        string_consts: HashMap::new(),
        float_consts: HashMap::new(),
        next_reg,
        max_reg: next_reg,
        locals,
        var_kinds,
        assigned,
        local_obj_uses: HashMap::new(),
        iterated_maps: HashSet::new(),
        map_method_regs: None,
        loops: Vec::new(),
        bails: Vec::new(),
        has_self_call: false,
        bound_regs: None,
    };
    if !builder.use_i32_bound {
        let r_lo = builder.load_f64(-(TIER_C_BOUND as f64))?;
        let r_hi = builder.load_f64(TIER_C_BOUND as f64)?;
        builder.bound_regs = Some((r_lo, r_hi));
        let r_mset = builder.alloc()?;
        let r_mget = builder.alloc()?;
        builder.map_method_regs = Some((r_mset, r_mget));
    }

    if !builder.emit_stmts(body)? {
        return Ok(None);
    }
    if is_void {
        // Normal fall-off for `: void` returns `0` (in-bounds sentinel); overflow bail returns `undefined`.
        let z = builder.alloc()?;
        builder.emit(Instr::LoadInt { dst: z, val: 0 })?;
        builder.emit(Instr::Return { src: z })?;
    }
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
    proto.string_constants = builder.string_constants;
    proto.is_strict = true;
    proto.non_constructable = true;
    proto.simple_params = true;
    let func_id = sink.push(proto)?;
    Ok(Some(FastMethodCompiled {
        func_id,
        arity: builder.param_count,
        returns_int: !is_void,
    }))
}
