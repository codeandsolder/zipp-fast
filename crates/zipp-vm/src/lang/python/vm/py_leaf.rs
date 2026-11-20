//! Frame-less execution of small acyclic Python leaf functions (`LeafBody`).
//!
//! Straight-line and forward-branching Python functions (arithmetic helpers,
//! default-argument wrappers, simple methods like `self.value += d; return
//! self.value`, conditional property setters like `if value < 0: raise ...;
//! self._cents = value`) are scanned once per `FuncProto` into [`LeafInfo`].
//! At a hot `PyCall` (both interpreter and compiled JIT regions) or property
//! accessor site, `Vm::py_leaf_exec` runs the body directly in a small stack
//! register file without pushing a `Frame`, resizing `Vm::regs`, or allocating
//! on the VM heap.
//!
//! ## Atomicity & GC safety
//!
//! * No instruction executed here allocates on the VM heap or invokes arbitrary
//!   guest code: `Vm::maybe_gc()` is never needed, stack registers cannot go
//!   stale, and JIT pinned pointers remain valid.
//! * Attribute stores (`PySetAttr`) are pre-validated against the callee's own
//!   layout-mode cache entries and buffered in a stack array, committed only
//!   when `Return` is reached. Any bail (overflow past 48-bit immediate int,
//!   non-numeric operand, cache miss, slow edge, or hitting a `raise`/call
//!   branch) leaves VM state completely untouched so the caller transparently
//!   falls back to pushing the real frame.

use super::super::py_ops::py_small_arith;
use super::super::py_rt::{ic, py_frame_guard_start, PyIc};
use super::super::*;
use crate::bytecode::{Instr, PyArithOp};
use crate::heap::HeapObj;
use crate::value::Value;

/// Maximum register index used in the main body of a leaf function (cold
/// slow-path blocks never execute here, so `proto.reg_count` may exceed this).
const LEAF_MAX_REGS: usize = 48;
/// Maximum parameter count of a leaf body.
const LEAF_MAX_PARAMS: usize = 12;
/// Maximum main-body instruction count scanned for a leaf body.
const LEAF_MAX_CODE: usize = 64;
/// Maximum buffered attribute writes per leaf call.
const LEAF_MAX_WRITES: usize = 4;

env_off_switch! {
    /// Whether leaf bodies run without pushing a frame (`Vm::py_leaf_exec`).
    /// On unless `ZIPP_NO_PY_LEAF_CALLS` is set. Read once per process.
    fn py_leaf_calls_enabled() = "ZIPP_NO_PY_LEAF_CALLS"
}

/// Cached analysis result for one `FuncProto`.
#[derive(Clone, Copy, Default)]
pub(crate) enum LeafSlot {
    #[default]
    Unread,
    No,
    Yes(LeafInfo),
}

/// Static facts about an acyclic leaf `FuncProto`.
#[derive(Clone, Copy)]
pub(crate) struct LeafInfo {
    pub main_end: u16,
    pub params: u8,
    /// True when the function has no call instructions outside raise-only
    /// branches, making call sites targeting it eligible for `py_call_site_native`.
    pub native_eligible: bool,
}

#[derive(Clone, Copy)]
struct PendingAttr {
    obj_bits: u64,
    dict_idx: u32,
    key: u32,
    pos: u32,
    next_layout: u32,
    is_append: bool,
    val: Value,
}

impl PendingAttr {
    const EMPTY: Self = Self {
        obj_bits: 0,
        dict_idx: 0,
        key: 0,
        pos: 0,
        next_layout: 0,
        is_append: false,
        val: Value::UNDEFINED,
    };
}

/// Scan a `FuncProto` once to see if its main body (before frame guard / cold
/// blocks) is an acyclic leaf function with at least one non-allocating path
/// to `Return`.
fn py_leaf_scan(p: &crate::bytecode::FuncProto) -> Option<LeafInfo> {
    if !py_leaf_calls_enabled()
        || p.is_generator
        || p.is_async
        || !p.is_strict
        || p.lexical_this
        || p.rest_reg.is_some()
        || p.arguments_reg.is_some()
        || (p.param_count as usize) > LEAF_MAX_PARAMS
    {
        return None;
    }
    let code = p.code.as_slice();
    if code.is_empty() {
        return None;
    }
    let main_end = py_frame_guard_start(p)
        .map(|s| (s as usize).min(code.len()))
        .unwrap_or(code.len());
    if main_end == 0 || main_end > LEAF_MAX_CODE {
        return None;
    }

    // Reject if any jump inside the main body targets `<= ip` (has a loop).
    for (ip, instr) in code[..main_end].iter().enumerate() {
        let target = match *instr {
            Instr::Jump { target }
            | Instr::JumpIfFalse { target, .. }
            | Instr::JumpIfTrue { target, .. }
            | Instr::JumpIfNotLt { target, .. }
            | Instr::JumpIfNotLe { target, .. }
            | Instr::PyJumpCompare { target, .. } => Some(target as usize),
            Instr::PushHandler { .. }
            | Instr::PushFinally { .. }
            | Instr::Yield { .. }
            | Instr::YieldDelegate { .. } => return None,
            _ => None,
        };
        if let Some(t) = target {
            if t <= ip {
                return None;
            }
        }
    }

    // Forward reachability over leaf-supported ops: prove at least one
    // Return is reachable from ip 0 without passing through an unsupported op.
    let mut reach = [false; LEAF_MAX_CODE];
    reach[0] = true;
    let mut has_return = false;
    let mut has_call = false;
    let mut reg_oob = false;
    let mut chk = |r: u16| {
        if (r as usize) >= LEAF_MAX_REGS {
            reg_oob = true;
        }
    };

    for (ip, instr) in code[..main_end].iter().enumerate() {
        if matches!(
            *instr,
            Instr::Call { .. }
                | Instr::CallMethod { .. }
                | Instr::CallWithThis { .. }
                | Instr::PyCall { .. }
        ) {
            has_call = true;
        }
        if !reach[ip] {
            continue;
        }
        let mut mark = |next: usize| {
            if next < main_end {
                reach[next] = true;
            }
        };
        match *instr {
            Instr::LoadGlobal { dst, .. }
            | Instr::LoadNull { dst }
            | Instr::LoadUndefined { dst }
            | Instr::LoadBool { dst, .. }
            | Instr::LoadInt { dst, .. }
            | Instr::LoadConst { dst, .. } => {
                chk(dst);
                mark(ip + 1);
            }
            Instr::Move { dst, src } => {
                chk(dst);
                chk(src);
                mark(ip + 1);
            }
            Instr::TypeOfIs { dst, a, .. }
            | Instr::Not { dst, a }
            | Instr::Neg { dst, a }
            | Instr::BitNot { dst, a }
            | Instr::AddInt { dst, a, .. }
            | Instr::PyAddImm { dst, a, .. } => {
                chk(dst);
                chk(a);
                mark(ip + 1);
            }
            Instr::Eq { dst, a, b }
            | Instr::Ne { dst, a, b }
            | Instr::PyArith { dst, a, b, .. }
            | Instr::PyCompare { dst, a, b, .. } => {
                chk(dst);
                chk(a);
                chk(b);
                mark(ip + 1);
            }
            Instr::PyGetAttr { dst, obj, .. } => {
                chk(dst);
                chk(obj);
                mark(ip + 1);
            }
            Instr::PySetAttr { obj, val, .. } => {
                chk(obj);
                chk(val);
                mark(ip + 1);
            }
            Instr::PyGetItem { dst, o, k, seq, dict, .. } => {
                chk(dst);
                chk(o);
                chk(k);
                chk(seq);
                chk(dict);
                mark(ip + 1);
            }
            Instr::PyLen { dst, v, rt, .. } => {
                chk(dst);
                chk(v);
                chk(rt);
                mark(ip + 1);
            }
            Instr::LoadBigInt { dst, value } if Value::small_bigint(value.get()).is_some() => {
                chk(dst);
                mark(ip + 1);
            }
            Instr::Jump { target } => mark(target as usize),
            Instr::JumpIfFalse { cond, target } | Instr::JumpIfTrue { cond, target } => {
                chk(cond);
                mark(ip + 1);
                mark(target as usize);
            }
            Instr::JumpIfNotLt { a, b, target }
            | Instr::JumpIfNotLe { a, b, target }
            | Instr::PyJumpCompare { a, b, target, .. } => {
                chk(a);
                chk(b);
                mark(ip + 1);
                mark(target as usize);
            }
            Instr::Return { src } => {
                chk(src);
                has_return = true;
            }
            _ => {}
        }
    }

    if !has_return || reg_oob {
        return None;
    }

    // Check if any call instruction sits on a path that can reach a Return
    // (as opposed to only appearing inside a `if ...: raise Exc(...)` branch).
    let native_eligible = if !has_call {
        true
    } else {
        let mut can_return = [false; LEAF_MAX_CODE];
        for ip in (0..main_end).rev() {
            can_return[ip] = match code[ip] {
                Instr::Return { .. } => true,
                Instr::PyRaise { .. } | Instr::Throw { .. } => false,
                Instr::Jump { target } => (target as usize) < main_end && can_return[target as usize],
                Instr::JumpIfFalse { target, .. }
                | Instr::JumpIfTrue { target, .. }
                | Instr::JumpIfNotLt { target, .. }
                | Instr::JumpIfNotLe { target, .. }
                | Instr::PyJumpCompare { target, .. } => {
                    (ip + 1 < main_end && can_return[ip + 1])
                        || ((target as usize) < main_end && can_return[target as usize])
                }
                _ => ip + 1 < main_end && can_return[ip + 1],
            };
        }
        !code[..main_end].iter().enumerate().any(|(ip, i)| {
            can_return[ip]
                && matches!(
                    *i,
                    Instr::Call { .. }
                        | Instr::CallMethod { .. }
                        | Instr::CallWithThis { .. }
                        | Instr::PyCall { .. }
                )
        })
    };

    Some(LeafInfo {
        main_end: main_end as u16,
        params: p.param_count as u8,
        native_eligible,
    })
}

#[inline(always)]
fn leaf_arith_num(op: PyArithOp, va: Value, vb: Value) -> Option<Value> {
    Some(match op {
        PyArithOp::Add => {
            if va.is_int() && vb.is_int() {
                match va.as_int().checked_add(vb.as_int()) {
                    Some(v) => Value::int(v),
                    None => Value::num(va.as_int() as f64 + vb.as_int() as f64),
                }
            } else {
                Value::num(va.as_f64() + vb.as_f64())
            }
        }
        PyArithOp::Sub => {
            if va.is_int() && vb.is_int() {
                match va.as_int().checked_sub(vb.as_int()) {
                    Some(v) => Value::int(v),
                    None => Value::num(va.as_int() as f64 - vb.as_int() as f64),
                }
            } else {
                Value::num(va.as_f64() - vb.as_f64())
            }
        }
        PyArithOp::Mul => {
            if va.is_int() && vb.is_int() {
                let (ia, ib) = (va.as_int(), vb.as_int());
                match ia.checked_mul(ib) {
                    Some(v) if v != 0 || (ia | ib) >= 0 => Value::int(v),
                    _ => Value::num(ia as f64 * ib as f64),
                }
            } else {
                Value::num(va.as_f64() * vb.as_f64())
            }
        }
        PyArithOp::TrueDiv => {
            let y = vb.as_f64();
            if y == 0.0 {
                return None;
            }
            Value::num(va.as_f64() / y)
        }
        _ => return None,
    })
}

impl<'p> Vm<'p> {
    /// Cached [`LeafInfo`] for `fid`, scanned once per code object.
    #[inline]
    pub(crate) fn py_leaf_info(&mut self, fid: u32) -> Option<LeafInfo> {
        let f = fid as usize;
        let known = self.py_rt.as_deref()?.leaf_bodies.get(f).copied().unwrap_or(LeafSlot::Unread);
        match known {
            LeafSlot::Yes(info) => Some(info),
            LeafSlot::No => None,
            LeafSlot::Unread => {
                let scanned = py_leaf_scan(self.func(f));
                let slot = match scanned {
                    Some(info) => LeafSlot::Yes(info),
                    None => LeafSlot::No,
                };
                if let Some(p) = self.py_rt.as_deref_mut() {
                    if p.leaf_bodies.len() <= f {
                        p.leaf_bodies.resize(f + 1, LeafSlot::Unread);
                    }
                    p.leaf_bodies[f] = slot;
                }
                scanned
            }
        }
    }

    /// Check whether `PySetAttr` at `(fid, ip)` can execute against `obj` in
    /// layout mode (`ATTR_SET_L` or `ATTR_APPEND_L`) without allocating or
    /// running guest code; returns `(dict_idx, pos, next_layout, is_append)`.
    #[inline]
    fn py_leaf_set_check(&self, fid: u32, ip: usize, obj: Value) -> Option<(u32, u32, u32, bool)> {
        if !obj.is_heap() {
            return None;
        }
        let e = self.py_ic(fid, ip);
        if e.kind != ic::ATTR_SET_L && e.kind != ic::ATTR_APPEND_L {
            return None;
        }
        let k = self.py_hit_keys()?;
        let (cls, dict) = self.py_hit_parts(k, obj)?;
        if cls.bits() != e.a || !cls.is_heap() || !dict.is_heap() {
            return None;
        }
        if !self.py_hit_stamp(k, cls, e.hver, e.c) {
            return None;
        }
        let (l, n) = self.py_attrs_of(dict)?;
        match e.kind {
            ic::ATTR_SET_L if l == e.d && (e.pos as usize) < n => {
                Some((dict.heap_index(), e.pos, l, false))
            }
            ic::ATTR_APPEND_L if l == e.d && (e.pos as usize) == n => {
                Some((dict.heap_index(), e.pos, e.b as u32, true))
            }
            _ => None,
        }
    }

    /// Read `PyGetAttr` at `(fid, ip)` on `obj` in a leaf body without
    /// allocating or pushing a frame: supports `ATTR_GET_L`, `GET_CLASS_L`,
    /// and trivial `@property` getters (`ATTR_PROP` -> `py_prop_inline_plan`).
    #[inline]
    fn py_leaf_attr_get(&self, fid: u32, ip: usize, obj: Value) -> Option<Value> {
        if !obj.is_heap() {
            return None;
        }
        let e = self.py_ic(fid, ip);
        let k = self.py_hit_keys()?;
        let (cls, dict) = self.py_hit_parts(k, obj)?;
        if cls.bits() != e.a || !cls.is_heap() || !dict.is_heap() {
            return None;
        }
        match e.kind {
            ic::ATTR_GET_L => {
                let v = self.py_attrs_at(dict, e.d, e.pos as usize)?;
                self.py_hit_stamp(k, cls, e.hver, e.c).then_some(v)
            }
            ic::GET_CLASS_L => {
                if self.py_attrs_of(dict).is_some_and(|(l, _)| l == e.d)
                    && self.py_hit_stamp(k, cls, e.hver, e.c)
                {
                    let ty = self.py_rt.as_deref()?.ty?;
                    let v = self.py_table_at(cls, ty.gv, e.b, e.pos)?;
                    (!v.is_undefined() && v != Value::HOLE).then_some(v)
                } else {
                    None
                }
            }
            ic::ATTR_PROP => {
                if !self.py_hit_stamp(k, cls, e.hver, e.c) {
                    return None;
                }
                let (layout, slot) = self.py_prop_inline_plan(cls, false, &e)?;
                self.py_attrs_at(dict, layout, slot as usize)
            }
            _ => None,
        }
    }

    /// Fast check whether `fid` is already cached as non-rec (`PyRecSlot::No`).
    #[cfg(all(feature = "jit", target_arch = "x86_64"))]
    #[inline]
    pub(crate) fn py_rec_is_no(&self, fid: u32) -> bool {
        matches!(
            self.py_rt.as_deref().and_then(|p| p.rec_jits.get(fid as usize)),
            Some(crate::codegen::PyRecSlot::No)
        )
    }

    /// Execute the leaf code object `fid` (with function record `fv` and
    /// positional `args`) directly in a stack register file without pushing a
    /// frame or allocating on the VM heap. Returns `None` with zero side
    /// effects whenever anything cannot complete purely here.
    #[inline(never)]
    pub(crate) fn py_leaf_exec(&mut self, fid: u32, fv: Value, args: &[Value]) -> Option<Value> {
        if super::prof_py::on() || !self.native_kernel_admits(64, 0) {
            return None;
        }
        let info = self.py_leaf_info(fid)?;
        let params = info.params as usize;
        let argc = args.len();
        if argc > params {
            return None;
        }

        let mut regs = [Value::UNDEFINED; LEAF_MAX_REGS];
        regs[0] = fv;
        regs[1..1 + argc].copy_from_slice(args);
        self.py_leaf_run(fid, fv, info, argc, &mut regs)
    }

    /// Variant of [`Vm::py_leaf_exec`] reading positional arguments directly
    /// from the caller's register window (avoiding a second stack copy).
    #[inline(never)]
    pub(crate) fn py_leaf_exec_window(&mut self, fid: u32, fv: Value, base: usize, arg_base: u16, argc: u16) -> Option<Value> {
        if super::prof_py::on() || !self.native_kernel_admits(64, 0) {
            return None;
        }
        let info = self.py_leaf_info(fid)?;
        let params = info.params as usize;
        let n = argc as usize;
        if n > params {
            return None;
        }
        let mut regs = [Value::UNDEFINED; LEAF_MAX_REGS];
        regs[0] = fv;
        let start = base + arg_base as usize;
        regs[1..1 + n].copy_from_slice(&self.regs[start..start + n]);
        self.py_leaf_run(fid, fv, info, n, &mut regs)
    }

    #[inline(always)]
    fn py_leaf_run(&mut self, fid: u32, fv: Value, info: LeafInfo, argc: usize, regs: &mut [Value; LEAF_MAX_REGS]) -> Option<Value> {
        let params = info.params as usize;
        // Fill missing positional parameters from `fv.defaults` (same check as
        // `Vm::py_fill_defaults`), so the prologue's default check skips cleanly.
        if argc < params {
            if !fv.is_heap() {
                return None;
            }
            let d = self.py_hint_field(super::super::py_rt::hint::DEFAULTS, fv.heap_index(), "defaults")?;
            if !d.is_heap() || (!self.array_js_len.is_empty() && self.array_js_len.contains_key(&d.heap_index())) {
                return None;
            }
            let HeapObj::Array(items) = self.heap.get(d.heap_index()) else {
                return None;
            };
            let first = params.checked_sub(items.len())?;
            if argc < first {
                return None;
            }
            for i in argc..params {
                let v = items[i - first];
                if v == Value::HOLE || v.is_undefined() {
                    return None;
                }
                regs[1 + i] = v;
            }
        }

        let main_end = info.main_end as usize;
        let mut pending = [PendingAttr::EMPTY; LEAF_MAX_WRITES];
        let mut n_pending = 0usize;
        let mut ip = 0usize;
        let mut steps = 0i64;

        while ip < main_end {
            steps += 1;
            if steps > 64 {
                return None;
            }
            let instr = self.func(fid as usize).code[ip];
            match instr {
                Instr::LoadGlobal { dst, idx } => {
                    let v = *self.globals.get(idx as usize)?;
                    if v == Value::UNINITIALIZED {
                        return None;
                    }
                    regs[dst as usize] = v;
                    ip += 1;
                }
                Instr::Move { dst, src } => {
                    regs[dst as usize] = regs[src as usize];
                    ip += 1;
                }
                Instr::LoadNull { dst } => {
                    regs[dst as usize] = Value::NULL;
                    ip += 1;
                }
                Instr::LoadUndefined { dst } => {
                    regs[dst as usize] = Value::UNDEFINED;
                    ip += 1;
                }
                Instr::LoadBool { dst, val } => {
                    regs[dst as usize] = Value::bool(val);
                    ip += 1;
                }
                Instr::LoadInt { dst, val } => {
                    regs[dst as usize] = Value::int(val);
                    ip += 1;
                }
                Instr::LoadBigInt { dst, value } => {
                    regs[dst as usize] = Value::small_bigint(value.get())?;
                    ip += 1;
                }
                Instr::LoadConst { dst, idx } => {
                    let raw = *self.func(fid as usize).constants.get(idx as usize)?;
                    if raw.is_heap() && (raw.heap_index() & crate::vm::helpers_misc::STRING_CONST_BIT) != 0 {
                        return None;
                    }
                    regs[dst as usize] = raw;
                    ip += 1;
                }
                Instr::TypeOfIs { dst, a, code, neg } => {
                    let v = regs[a as usize];
                    let eq = if let Some(&want) = crate::bytecode::TYPEOF_NAMES.get(code as usize) {
                        let got = if v.is_undefined() {
                            "undefined"
                        } else if v.is_small_bigint() {
                            "bigint"
                        } else if v.is_number() {
                            "number"
                        } else if v == Value::TRUE || v == Value::FALSE {
                            "boolean"
                        } else {
                            self.type_of(v)
                        };
                        want == got
                    } else {
                        false
                    };
                    regs[dst as usize] = Value::bool(eq != neg);
                    ip += 1;
                }
                Instr::Not { dst, a } => {
                    let v = regs[a as usize];
                    if v == Value::TRUE {
                        regs[dst as usize] = Value::FALSE;
                    } else if v == Value::FALSE {
                        regs[dst as usize] = Value::TRUE;
                    } else {
                        return None;
                    }
                    ip += 1;
                }
                Instr::Neg { dst, a } => {
                    let v = regs[a as usize];
                    regs[dst as usize] = if let Some(n) = v.small_bigint_val() {
                        Value::small_bigint(-(n as i128))?
                    } else if v.is_number() {
                        Value::num(-v.as_f64())
                    } else {
                        return None;
                    };
                    ip += 1;
                }
                Instr::BitNot { dst, a } => {
                    let n = regs[a as usize].small_bigint_val()?;
                    regs[dst as usize] = Value::small_bigint(!(n as i128))?;
                    ip += 1;
                }
                Instr::AddInt { dst, a, imm, upd } => {
                    let va = regs[a as usize];
                    regs[dst as usize] = if va.is_int() {
                        match va.as_int().checked_add(imm) {
                            Some(v) => Value::int(v),
                            None => Value::num(va.as_int() as f64 + imm as f64),
                        }
                    } else if va.is_double() {
                        Value::num(va.as_f64() + imm as f64)
                    } else if upd {
                        let x = va.small_bigint_val()?;
                        Value::small_bigint((x as i128) + (imm as i128))?
                    } else {
                        return None;
                    };
                    ip += 1;
                }
                Instr::Eq { dst, a, b } | Instr::Ne { dst, a, b } => {
                    let (va, vb) = (regs[a as usize], regs[b as usize]);
                    // Exact JS `===` / `!==` for singletons, ints, and small BigInts
                    // (Python `is` / `is not` emits Eq/Ne for None/True/False).
                    let eq = if va.bits() == vb.bits() {
                        !(va.is_double() && va.as_f64().is_nan())
                    } else if va.is_number() && vb.is_number() {
                        va.as_f64() == vb.as_f64()
                    } else if (va == Value::NULL || va == Value::TRUE || va == Value::FALSE || va.is_small_bigint())
                        && (vb == Value::NULL || vb == Value::TRUE || vb == Value::FALSE || vb.is_small_bigint())
                    {
                        false
                    } else {
                        return None;
                    };
                    let is_ne = matches!(instr, Instr::Ne { .. });
                    regs[dst as usize] = Value::bool(eq != is_ne);
                    ip += 1;
                }
                Instr::PyAddImm { dst, a, imm, .. } => {
                    let va = regs[a as usize];
                    regs[dst as usize] = if let Some(x) = va.small_bigint_val() {
                        Value::small_bigint((x as i128) + (imm as i128))?
                    } else if va.is_number() {
                        Value::num(va.as_f64() + imm as f64)
                    } else {
                        return None;
                    };
                    ip += 1;
                }
                Instr::PyArith { op, dst, a, b, .. } => {
                    let (va, vb) = (regs[a as usize], regs[b as usize]);
                    let res = if let (Some(x), Some(y)) = (va.small_bigint_val(), vb.small_bigint_val()) {
                        let r = py_small_arith(op, x, y)?;
                        Value::small_bigint(r as i128)?
                    } else if va.is_number() && vb.is_number() {
                        leaf_arith_num(op, va, vb)?
                    } else {
                        return None;
                    };
                    regs[dst as usize] = res;
                    ip += 1;
                }
                Instr::PyCompare { op, dst, a, b, .. } => {
                    let r = self.py_compare(op, regs[a as usize], regs[b as usize]).ok()??;
                    regs[dst as usize] = Value::bool(r);
                    ip += 1;
                }
                Instr::PyJumpCompare { op, a, b, when, target, .. } => {
                    let r = self.py_compare(op, regs[a as usize], regs[b as usize]).ok()??;
                    if r == when {
                        ip = target as usize;
                    } else {
                        ip += 1;
                    }
                }
                Instr::Jump { target } => {
                    ip = target as usize;
                }
                Instr::JumpIfFalse { cond, target } => {
                    let v = regs[cond as usize];
                    if v == Value::FALSE {
                        ip = target as usize;
                    } else if v == Value::TRUE {
                        ip += 1;
                    } else {
                        return None;
                    }
                }
                Instr::JumpIfTrue { cond, target } => {
                    let v = regs[cond as usize];
                    if v == Value::TRUE {
                        ip = target as usize;
                    } else if v == Value::FALSE {
                        ip += 1;
                    } else {
                        return None;
                    }
                }
                Instr::JumpIfNotLt { a, b, target } => {
                    let (va, vb) = (regs[a as usize], regs[b as usize]);
                    let lt = if let (Some(x), Some(y)) = (va.small_bigint_val(), vb.small_bigint_val()) {
                        x < y
                    } else if va.is_number() && vb.is_number() {
                        va.as_f64() < vb.as_f64()
                    } else {
                        return None;
                    };
                    if !lt {
                        ip = target as usize;
                    } else {
                        ip += 1;
                    }
                }
                Instr::JumpIfNotLe { a, b, target } => {
                    let (va, vb) = (regs[a as usize], regs[b as usize]);
                    let le = if let (Some(x), Some(y)) = (va.small_bigint_val(), vb.small_bigint_val()) {
                        x <= y
                    } else if va.is_number() && vb.is_number() {
                        va.as_f64() <= vb.as_f64()
                    } else {
                        return None;
                    };
                    if !le {
                        ip = target as usize;
                    } else {
                        ip += 1;
                    }
                }
                Instr::PyGetAttr { dst, obj, key, .. } => {
                    let ov = regs[obj as usize];
                    let mut hit = None;
                    if n_pending > 0 {
                        let ob = ov.bits();
                        for w in pending[..n_pending].iter().rev() {
                            if w.obj_bits == ob {
                                if w.key == key
                                    || self
                                        .py_const_name(fid, w.key)
                                        .zip(self.py_const_name(fid, key))
                                        .is_some_and(|(a, b)| a == b)
                                {
                                    hit = Some(w.val);
                                    break;
                                }
                                if w.is_append {
                                    return None;
                                }
                            }
                        }
                    }
                    let v = match hit {
                        Some(v) => v,
                        None => self.py_leaf_attr_get(fid, ip, ov)?,
                    };
                    regs[dst as usize] = v;
                    ip += 1;
                }
                Instr::PySetAttr { obj, key, val, .. } => {
                    if n_pending >= LEAF_MAX_WRITES {
                        return None;
                    }
                    let ov = regs[obj as usize];
                    let vv = regs[val as usize];
                    let (dict_idx, pos, next_layout, is_append) = self.py_leaf_set_check(fid, ip, ov)?;
                    // Keep layout transitions unambiguous: if an earlier pending
                    // write already mutated this dict's layout, bail cleanly.
                    if n_pending > 0
                        && pending[..n_pending]
                            .iter()
                            .any(|w| w.dict_idx == dict_idx && (w.is_append || is_append))
                    {
                        return None;
                    }
                    pending[n_pending] = PendingAttr {
                        obj_bits: ov.bits(),
                        dict_idx,
                        key,
                        pos,
                        next_layout,
                        is_append,
                        val: vv,
                    };
                    n_pending += 1;
                    ip += 1;
                }
                Instr::PyGetItem { dst, o, k, seq, dict, .. } => {
                    let Some(Some(v)) = self
                        .py_get_item(regs[o as usize], regs[k as usize], regs[seq as usize], regs[dict as usize])
                        .ok()?
                    else {
                        return None;
                    };
                    regs[dst as usize] = v;
                    ip += 1;
                }
                Instr::PyLen { dst, v, rt, .. } => {
                    let n = self.py_len(regs[v as usize], regs[rt as usize])?;
                    regs[dst as usize] = Value::small_bigint(n as i128)?;
                    ip += 1;
                }
                Instr::Return { src } => {
                    let ret = regs[src as usize];
                    for w in &pending[..n_pending] {
                        if w.is_append {
                            self.py_attrs_push(w.dict_idx, w.next_layout, w.val);
                        } else {
                            self.py_attrs_put(w.dict_idx, w.pos as usize, w.val);
                        }
                    }
                    if self.instr_rec.is_some() {
                        self.charge_steps(steps);
                    }
                    return Some(ret);
                }
                _ => return None,
            }
        }
        None
    }

    /// Execute a `PyCall` site whose cache entry `e` names a plain Python
    /// callee (`e.c != 0`) directly via [`Vm::py_leaf_exec`], without pushing
    /// a frame. Used by compiled code (`jit_py_call`).
    #[cfg(all(feature = "jit", target_arch = "x86_64"))]
    #[inline]
    pub(in crate::vm) fn py_leaf_call_site(&mut self, e: &PyIc, f: Value, base: usize, arg_base: u16, argc: u16) -> Option<Value> {
        if e.kind != ic::CALL_LEAF || e.c == 0 || e.a != f.bits() || !f.is_heap() {
            return None;
        }
        let (fid, params) = (e.c as u32, (e.c >> 48) as u16);
        if argc > params || (argc as usize) > LEAF_MAX_PARAMS {
            return None;
        }
        // Quick reject before touching the function record if already known non-leaf.
        if let Some(p) = self.py_rt.as_deref() {
            if matches!(p.leaf_bodies.get(fid as usize), Some(LeafSlot::No)) {
                return None;
            }
        }
        let fi = f.heap_index();
        if self.heap.version_of(fi) != e.hver {
            return None;
        }
        let pos = e.pos as usize;
        match self.heap.get(fi) {
            HeapObj::Object(m) if pos < m.slot_len() && !m.is_accessor_at(pos) && m.val_at(pos).bits() == e.b => {}
            _ => return None,
        }
        self.py_leaf_exec_window(fid, f, base, arg_base, argc)
    }

    /// Execute a pure small-int self-recursive Python function `fid` via its
    /// compiled x86-64 micro-JIT body (`jit::py_rec`), returning `Some(val)`
    /// on completion or `None` (zero side effects) to fall back to the
    /// interpreter.
    #[cfg(all(feature = "jit", target_arch = "x86_64"))]
    #[inline(never)]
    pub(crate) fn py_rec_exec(&mut self, fid: u32, fv: Value, args: &[Value]) -> Option<Value> {
        use crate::codegen::{compile_py_rec, py_rec_enabled, PyRecCtx, PyRecSlot, REC_MAX_DEPTH};
        if !py_rec_enabled()
            || super::prof_py::on()
            || self.frames.len() >= 16
            || !self.native_kernel_admits(64, 0)
            || args.is_empty()
            || args.len() > 4
        {
            return None;
        }
        let f = fid as usize;
        // Compile on first sight (cached in `PyRt::rec_jits`).
        let needs_compile = matches!(
            self.py_rt.as_deref()?.rec_jits.get(f),
            None | Some(PyRecSlot::Unread)
        );
        if needs_compile {
            let compiled = compile_py_rec(self.func(f));
            let slot = match compiled {
                Some(c) => PyRecSlot::Yes(Box::new(c)),
                None => PyRecSlot::No,
            };
            let p = self.py_rt.as_deref_mut()?;
            if p.rec_jits.len() <= f {
                p.rec_jits.resize_with(f + 1, PyRecSlot::default);
            }
            p.rec_jits[f] = slot;
        }
        let (entry, params, global_ips, n_globals, rt) = {
            let p = self.py_rt.as_deref()?;
            let Some(PyRecSlot::Yes(rec)) = p.rec_jits.get(f) else {
                return None;
            };
            (rec.entry, rec.params as usize, rec.global_ips, rec.n_globals as usize, p.r)
        };
        if args.len() != params {
            return None;
        }
        // Unbox all arguments as small immediate BigInts first (cheap reject).
        let mut a = [0i64; 4];
        for i in 0..params {
            a[i] = args[i].small_bigint_val()?;
        }
        // Verify every self-call global binding still resolves to `fv` (exact
        // semantics even if the function name is rebound in module globals).
        for &gip in &global_ips[..n_globals] {
            let gip = gip as usize;
            let bound = match self.py_global_hit(fid, gip, fv, rt) {
                Some(v) => v,
                None => {
                    let key = match self.func(f).code.get(gip) {
                        Some(&Instr::PyGlobal { key, .. }) => key,
                        _ => return None,
                    };
                    self.py_global(fid, gip, fv, rt, key)?
                }
            };
            if bound.bits() != fv.bits() {
                return None;
            }
        }
        let init_budget = self
            .instr_rec
            .as_ref()
            .and_then(|r| r.finite_remaining(self.meter_hot()))
            .map(|left| left.min(100_000_000) as i64)
            .unwrap_or(100_000_000);
        if init_budget <= 64 {
            return None;
        }
        let mut ctx = PyRecCtx {
            budget: init_budget,
            depth_left: REC_MAX_DEPTH,
        };
        let res = entry(a[0], a[1], a[2], a[3], &mut ctx);
        if res == i64::MIN {
            return None;
        }
        let boxed = Value::small_bigint(res as i128)?;
        let spent = (init_budget - ctx.budget).max(1);
        self.charge_steps(spent);
        Some(boxed)
    }

    /// Compile and cache `rec_jits[fid]` if not yet attempted; returns whether
    /// `fid` has a compiled recursive micro-JIT body.
    #[cfg(all(feature = "jit", target_arch = "x86_64"))]
    pub(crate) fn py_rec_prepare(&mut self, fid: u32) -> bool {
        use crate::codegen::{compile_py_rec, py_rec_enabled, PyRecSlot};
        if !py_rec_enabled() {
            return false;
        }
        let f = fid as usize;
        let known = self.py_rt.as_deref().and_then(|p| p.rec_jits.get(f));
        match known {
            Some(PyRecSlot::Yes(_)) => return true,
            Some(PyRecSlot::No) => return false,
            _ => {}
        }
        let compiled = compile_py_rec(self.func(f));
        let yes = compiled.is_some();
        let slot = match compiled {
            Some(c) => PyRecSlot::Yes(Box::new(c)),
            None => PyRecSlot::No,
        };
        if let Some(p) = self.py_rt.as_deref_mut() {
            if p.rec_jits.len() <= f {
                p.rec_jits.resize_with(f + 1, PyRecSlot::default);
            }
            p.rec_jits[f] = slot;
        }
        yes
    }

    /// Execute a `PyCall` site via [`Vm::py_rec_exec`] when its cache entry
    /// names a plain Python callee (`e.c != 0`).
    #[cfg(all(feature = "jit", target_arch = "x86_64"))]
    #[inline]
    pub(in crate::vm) fn py_rec_call_site(&mut self, e: &PyIc, f: Value, base: usize, arg_base: u16, argc: u16) -> Option<Value> {
        if e.kind != ic::CALL_REC || e.c == 0 || e.a != f.bits() || !f.is_heap() || argc == 0 || argc > 4 {
            return None;
        }
        let (fid, params) = (e.c as u32, (e.c >> 48) as u16);
        if argc != params {
            return None;
        }
        if let Some(p) = self.py_rt.as_deref() {
            if matches!(p.rec_jits.get(fid as usize), Some(crate::codegen::PyRecSlot::No)) {
                return None;
            }
        }
        let fi = f.heap_index();
        if self.heap.version_of(fi) != e.hver {
            return None;
        }
        let pos = e.pos as usize;
        match self.heap.get(fi) {
            HeapObj::Object(m) if pos < m.slot_len() && !m.is_accessor_at(pos) && m.val_at(pos).bits() == e.b => {}
            _ => return None,
        }
        let mut argv = [Value::UNDEFINED; 4];
        let start = base + arg_base as usize;
        let n = argc as usize;
        argv[..n].copy_from_slice(&self.regs[start..start + n]);
        self.py_rec_exec(fid, f, &argv[..n])
    }
}
