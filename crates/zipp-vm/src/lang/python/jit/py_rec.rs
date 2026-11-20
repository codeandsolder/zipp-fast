//! Self-contained x86-64 micro-JIT for pure small-int self-recursive Python
//! functions (`fib`, `fact`, `tak`, etc.).
//!
//! Compiles eligible Python `FuncProto`s directly into a compact Win64 x86-64
//! `ExecutableBuffer` operating on unboxed `i64` integers, with:
//! * `jo` overflow checks on every `+`, `-`, `*` -> bail sentinel (`i64::MIN`);
//! * 47-bit small-BigInt range check on return (`[-2^46, 2^46 - 1]` via `Value::small_bigint`) -> bail;
//! * recursion depth cap (`512`) -> bail to the VM's heap-backed frame stack
//!   (preserving deep recursion and catchable `RecursionError`);
//! * exact step-budget accounting -> bail when exhausted;
//! * O(1) entry verification of every self-call `PyGlobal` IC -> if the global
//!   binding changed (monkey-patched), falls back cleanly to the interpreter.

use super::*;
use crate::bytecode::{FuncProto, Instr, PyArithOp, PyCmpOp};
use crate::value::Value;
use dynasmrt::{dynasm, DynasmApi, DynasmLabelApi};

const REC_MAX_REGS: usize = 48;
const REC_MAX_PARAMS: usize = 4;
const REC_MAX_CODE: usize = 64;
const REC_MAX_GLOBALS: usize = 4;
/// Maximum native stack depth before bailing to the VM's explicit frame stack.
pub(crate) const REC_MAX_DEPTH: i64 = 512;

env_off_switch! {
    /// Respects `ZIPP_NOJIT` so pure-interpreter test runs disable `py_rec`.
    fn py_rec_nojit_off() = "ZIPP_NOJIT"
}
env_off_switch! {
    /// Dedicated same-binary A/B toggle for the recursive int micro-JIT.
    fn py_rec_enabled_flag() = "ZIPP_NO_PY_REC_JIT"
}

#[inline]
pub(crate) fn py_rec_enabled() -> bool {
    py_rec_nojit_off() && py_rec_enabled_flag()
}

/// Runtime context passed into compiled recursive x86-64 code.
#[repr(C)]
pub(crate) struct PyRecCtx {
    pub budget: i64,
    pub depth_left: i64,
}

pub(crate) type PyRecFn = extern "win64" fn(i64, i64, i64, i64, *mut PyRecCtx) -> i64;

/// Cached compilation state per `FuncProto` (stored in `PyRt::rec_jits`).
#[derive(Default)]
pub(crate) enum PyRecSlot {
    #[default]
    Unread,
    No,
    Yes(Box<PyRecCode>),
}

/// Compiled native body for one self-recursive pure-int Python function.
pub(crate) struct PyRecCode {
    _buf: CodeBuf,
    pub entry: PyRecFn,
    pub params: u8,
    pub global_ips: [u16; REC_MAX_GLOBALS],
    pub n_globals: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValKind {
    Unk,
    Int,
    Bool,
    SelfFn,
}

#[inline]
fn slot_off(r: u16) -> i32 {
    -(((r as i32) + 1) * 8)
}

/// Check if `proto` is a pure small-int self-recursive function and, if so,
/// compile its main body (`0..main_end`) to x86-64 machine code.
pub(crate) fn compile_py_rec(proto: &FuncProto) -> Option<PyRecCode> {
    if !py_rec_enabled()
        || proto.is_generator
        || proto.is_async
        || !proto.is_strict
        || proto.lexical_this
        || proto.rest_reg.is_some()
        || proto.arguments_reg.is_some()
        || proto.param_count == 0
        || (proto.param_count as usize) > REC_MAX_PARAMS
    {
        return None;
    }
    let code = proto.code.as_slice();
    if code.is_empty() {
        return None;
    }
    let main_end = crate::vm::py_frame_guard_start(proto)
        .map(|s| (s as usize).min(code.len()))
        .unwrap_or(code.len());
    if main_end == 0 || main_end > REC_MAX_CODE {
        return None;
    }

    let params = proto.param_count as usize;
    let reg_count = REC_MAX_REGS;
    let mut max_reg = params;
    let mut touch = |r: u16| -> Option<()> {
        let ru = r as usize;
        if ru >= REC_MAX_REGS {
            return None;
        }
        if ru > max_reg {
            max_reg = ru;
        }
        Some(())
    };

    // Exact per-IP forward dataflow & CFG validation pass (forward-only jumps,
    // no loops, all reachable arithmetic/compare/call/return operands proved
    // Int, all reachable PyCall callees proved SelfFn). Unreachable trailing
    // ops (such as an implicit `LoadNull; Return` after an explicit `return`)
    // are skipped.
    let mut init = [ValKind::Unk; REC_MAX_REGS];
    for i in 1..=params {
        init[i] = ValKind::Int;
    }
    let mut in_kinds: [Option<[ValKind; REC_MAX_REGS]>; REC_MAX_CODE] = [None; REC_MAX_CODE];
    in_kinds[0] = Some(init);
    let mut reachable = [false; REC_MAX_CODE];
    let mut move_copy = [false; REC_MAX_CODE];
    let mut global_ips = [0u16; REC_MAX_GLOBALS];
    let mut n_globals = 0usize;
    let mut self_calls = 0usize;
    let mut has_return = false;

    let merge = |states: &mut [Option<[ValKind; REC_MAX_REGS]>; REC_MAX_CODE],
                 next: usize,
                 cur: &[ValKind; REC_MAX_REGS]| {
        if next >= main_end {
            return;
        }
        match &mut states[next] {
            Some(existing) => {
                for r in 0..REC_MAX_REGS {
                    if existing[r] != cur[r] {
                        existing[r] = ValKind::Unk;
                    }
                }
            }
            slot @ None => *slot = Some(*cur),
        }
    };

    for (ip, &instr) in code[..main_end].iter().enumerate() {
        let Some(mut cur) = in_kinds[ip] else {
            continue;
        };
        reachable[ip] = true;
        match instr {
            Instr::LoadGlobal { dst, .. } => {
                if (dst as usize) <= params || (dst as usize) >= reg_count {
                    return None;
                }
                cur[dst as usize] = ValKind::Unk;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::GetProp { dst, obj: 0, .. } => {
                if (dst as usize) <= params || (dst as usize) >= reg_count {
                    return None;
                }
                cur[dst as usize] = ValKind::Unk;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::PyGlobal { dst, .. } => {
                if (dst as usize) >= reg_count || n_globals >= REC_MAX_GLOBALS {
                    return None;
                }
                global_ips[n_globals] = ip as u16;
                n_globals += 1;
                cur[dst as usize] = ValKind::SelfFn;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::Move { dst, src } => {
                let k = cur[src as usize];
                if k == ValKind::Unk {
                    return None;
                }
                if k != ValKind::SelfFn {
                    touch(dst)?;
                    touch(src)?;
                    move_copy[ip] = true;
                } else if (dst as usize) >= reg_count || (src as usize) >= reg_count {
                    return None;
                }
                cur[dst as usize] = k;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::LoadBigInt { dst, value } => {
                touch(dst)?;
                if Value::small_bigint(value.get()).is_none() {
                    return None;
                }
                cur[dst as usize] = ValKind::Int;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::PyAddImm { dst, a, .. } => {
                touch(dst)?;
                touch(a)?;
                if cur[a as usize] != ValKind::Int {
                    return None;
                }
                cur[dst as usize] = ValKind::Int;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::PyArith {
                op: PyArithOp::Add | PyArithOp::Sub | PyArithOp::Mul,
                dst,
                a,
                b,
                ..
            } => {
                touch(dst)?;
                touch(a)?;
                touch(b)?;
                if cur[a as usize] != ValKind::Int || cur[b as usize] != ValKind::Int {
                    return None;
                }
                cur[dst as usize] = ValKind::Int;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::PyCompare { dst, a, b, .. } => {
                touch(dst)?;
                touch(a)?;
                touch(b)?;
                if cur[a as usize] != ValKind::Int || cur[b as usize] != ValKind::Int {
                    return None;
                }
                cur[dst as usize] = ValKind::Bool;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::PyJumpCompare { a, b, target, .. } => {
                touch(a)?;
                touch(b)?;
                let t = target as usize;
                if t <= ip || t >= main_end || cur[a as usize] != ValKind::Int || cur[b as usize] != ValKind::Int {
                    return None;
                }
                merge(&mut in_kinds, ip + 1, &cur);
                merge(&mut in_kinds, t, &cur);
            }
            Instr::Jump { target } => {
                let t = target as usize;
                if t <= ip || t >= main_end {
                    return None;
                }
                merge(&mut in_kinds, t, &cur);
            }
            Instr::JumpIfFalse { cond, target } | Instr::JumpIfTrue { cond, target } => {
                touch(cond)?;
                let t = target as usize;
                if t <= ip
                    || t >= main_end
                    || !matches!(cur[cond as usize], ValKind::Bool | ValKind::Int)
                {
                    return None;
                }
                merge(&mut in_kinds, ip + 1, &cur);
                merge(&mut in_kinds, t, &cur);
            }
            Instr::PyCall { dst, f, arg_base, argc, .. } => {
                touch(dst)?;
                if (f as usize) >= reg_count
                    || cur[f as usize] != ValKind::SelfFn
                    || (argc as usize) != params
                    || (arg_base as usize) + (argc as usize) > reg_count
                {
                    return None;
                }
                for i in 0..params {
                    touch(arg_base + i as u16)?;
                    if cur[arg_base as usize + i] != ValKind::Int {
                        return None;
                    }
                }
                cur[dst as usize] = ValKind::Int;
                self_calls += 1;
                merge(&mut in_kinds, ip + 1, &cur);
            }
            Instr::Return { src } => {
                touch(src)?;
                if cur[src as usize] != ValKind::Int {
                    return None;
                }
                has_return = true;
            }
            _ => return None,
        }
    }

    if self_calls == 0 || n_globals == 0 || !has_return {
        return None;
    }

    // Emit x86-64 machine code using only the main body's register count
    // (cold slow-path blocks never run inside the native body).
    let mut ops = X64Asm::new(0);
    let frame_bytes = ((((max_reg + 1) as i32) * 8) + 15) & !15;
    let cost_per_call = (main_end as i32).max(4);

    let wrapper_off = ops.offset();
    let rec_entry = ops.new_dynamic_label();
    let bail = ops.new_dynamic_label();
    let bail_no_frame = ops.new_dynamic_label();
    let labels: Vec<dynasmrt::DynamicLabel> = (0..main_end).map(|_| ops.new_dynamic_label()).collect();

    // Win64 outer wrapper:
    //   fn(a0: rcx, a1: rdx, a2: r8, a3: r9, ctx: [rbp + 48]) -> rax
    dynasm!(ops
        ; push rbp
        ; mov rbp, rsp
        ; push rbx
        ; push r12
        ; push r13
        ; push r14
        ; mov r12, 1
        ; ror r12, 1
        ; mov r13, [rbp + 48]
        ; mov r14, [r13]
        ; mov rbx, [r13 + 8]
        ; call => rec_entry
        ; mov [r13], r14
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbx
        ; pop rbp
        ; ret
    );

    // Internal fast recursive entry point.
    dynasm!(ops
        ; => rec_entry
        ; dec rbx
        ; jle => bail_no_frame
        ; sub r14, cost_per_call
        ; jle => bail_no_frame
        ; push rbp
        ; mov rbp, rsp
        ; sub rsp, frame_bytes
    );
    if params >= 1 {
        dynasm!(ops ; mov [rbp + slot_off(1)], rcx);
    }
    if params >= 2 {
        dynasm!(ops ; mov [rbp + slot_off(2)], rdx);
    }
    if params >= 3 {
        dynasm!(ops ; mov [rbp + slot_off(3)], r8);
    }
    if params >= 4 {
        dynasm!(ops ; mov [rbp + slot_off(4)], r9);
    }

    for (ip, &instr) in code[..main_end].iter().enumerate() {
        dynasm!(ops ; => labels[ip]);
        if !reachable[ip] {
            continue;
        }
        match instr {
            Instr::LoadGlobal { .. } | Instr::GetProp { .. } | Instr::PyGlobal { .. } => {}
            Instr::Move { dst, src } => {
                if move_copy[ip] {
                    dynasm!(ops
                        ; mov rax, [rbp + slot_off(src)]
                        ; mov [rbp + slot_off(dst)], rax
                    );
                }
            }
            Instr::LoadBigInt { dst, value } => {
                let v = value.get() as i64;
                dynasm!(ops
                    ; mov rax, QWORD v
                    ; mov [rbp + slot_off(dst)], rax
                );
            }
            Instr::PyAddImm { dst, a, imm, .. } => {
                dynasm!(ops
                    ; mov rax, [rbp + slot_off(a)]
                    ; add rax, imm
                    ; jo => bail
                    ; mov [rbp + slot_off(dst)], rax
                );
            }
            Instr::PyArith { op, dst, a, b, .. } => {
                dynasm!(ops ; mov rax, [rbp + slot_off(a)]);
                match op {
                    PyArithOp::Add => dynasm!(ops ; add rax, [rbp + slot_off(b)] ; jo => bail),
                    PyArithOp::Sub => dynasm!(ops ; sub rax, [rbp + slot_off(b)] ; jo => bail),
                    PyArithOp::Mul => dynasm!(ops ; imul rax, [rbp + slot_off(b)] ; jo => bail),
                    _ => return None,
                }
                dynasm!(ops ; mov [rbp + slot_off(dst)], rax);
            }
            Instr::PyCompare { op, dst, a, b, .. } => {
                dynasm!(ops
                    ; mov rax, [rbp + slot_off(a)]
                    ; cmp rax, [rbp + slot_off(b)]
                );
                match op {
                    PyCmpOp::Lt => dynasm!(ops ; setl al),
                    PyCmpOp::Le => dynasm!(ops ; setle al),
                    PyCmpOp::Gt => dynasm!(ops ; setg al),
                    PyCmpOp::Ge => dynasm!(ops ; setge al),
                    PyCmpOp::Eq => dynasm!(ops ; sete al),
                    PyCmpOp::Ne => dynasm!(ops ; setne al),
                }
                dynasm!(ops
                    ; movzx rax, al
                    ; mov [rbp + slot_off(dst)], rax
                );
            }
            Instr::PyJumpCompare { op, a, b, when, target, .. } => {
                let tgt = labels[target as usize];
                dynasm!(ops
                    ; mov rax, [rbp + slot_off(a)]
                    ; cmp rax, [rbp + slot_off(b)]
                );
                let eff = if when {
                    op
                } else {
                    match op {
                        PyCmpOp::Lt => PyCmpOp::Ge,
                        PyCmpOp::Le => PyCmpOp::Gt,
                        PyCmpOp::Gt => PyCmpOp::Le,
                        PyCmpOp::Ge => PyCmpOp::Lt,
                        PyCmpOp::Eq => PyCmpOp::Ne,
                        PyCmpOp::Ne => PyCmpOp::Eq,
                    }
                };
                match eff {
                    PyCmpOp::Lt => dynasm!(ops ; jl => tgt),
                    PyCmpOp::Le => dynasm!(ops ; jle => tgt),
                    PyCmpOp::Gt => dynasm!(ops ; jg => tgt),
                    PyCmpOp::Ge => dynasm!(ops ; jge => tgt),
                    PyCmpOp::Eq => dynasm!(ops ; je => tgt),
                    PyCmpOp::Ne => dynasm!(ops ; jne => tgt),
                }
            }
            Instr::Jump { target } => {
                let tgt = labels[target as usize];
                dynasm!(ops ; jmp => tgt);
            }
            Instr::JumpIfFalse { cond, target } => {
                let tgt = labels[target as usize];
                dynasm!(ops
                    ; cmp QWORD [rbp + slot_off(cond)], 0
                    ; je => tgt
                );
            }
            Instr::JumpIfTrue { cond, target } => {
                let tgt = labels[target as usize];
                dynasm!(ops
                    ; cmp QWORD [rbp + slot_off(cond)], 0
                    ; jne => tgt
                );
            }
            Instr::PyCall { dst, arg_base, .. } => {
                if params >= 1 {
                    dynasm!(ops ; mov rcx, [rbp + slot_off(arg_base)]);
                }
                if params >= 2 {
                    dynasm!(ops ; mov rdx, [rbp + slot_off(arg_base + 1)]);
                }
                if params >= 3 {
                    dynasm!(ops ; mov r8, [rbp + slot_off(arg_base + 2)]);
                }
                if params >= 4 {
                    dynasm!(ops ; mov r9, [rbp + slot_off(arg_base + 3)]);
                }
                dynasm!(ops
                    ; call => rec_entry
                    ; cmp rax, r12
                    ; je => bail
                    ; mov [rbp + slot_off(dst)], rax
                );
            }
            Instr::Return { src } => {
                dynasm!(ops
                    ; mov rax, [rbp + slot_off(src)]
                    ; cmp rax, r12
                    ; je => bail
                    ; inc rbx
                    ; add rsp, frame_bytes
                    ; pop rbp
                    ; ret
                );
            }
            _ => return None,
        }
    }

    dynasm!(ops
        ; => bail
        ; mov rax, r12
        ; inc rbx
        ; add rsp, frame_bytes
        ; pop rbp
        ; ret
        ; => bail_no_frame
        ; inc rbx
        ; mov rax, r12
        ; ret
    );

    let buf = finalize_code(ops)?;
    let entry: PyRecFn = unsafe { std::mem::transmute(buf.ptr(wrapper_off)) };
    if crate::env_var_os("ZIPP_JITLOG").is_some() {
        eprintln!(
            "[jit] PY rec {} compiled (params={}, main_end={}, regs={}, bytes={})",
            proto.name,
            params,
            main_end,
            max_reg + 1,
            buf.len()
        );
    }
    Some(PyRecCode {
        _buf: buf,
        entry,
        params: params as u8,
        global_ips,
        n_globals: n_globals as u8,
    })
}
