//! PHP-to-ZIPP register bytecode emitter (uses only base `Instr` opcodes).
//!
//! ABI of every compiled PHP function / method / closure / file body:
//!   reg 0 (`REG_FN`):    `this` — the PHP function/context record (holds `.uses`, `.statics`)
//!   reg 1 (`REG_ARGS`):  JS Array of bound arguments (by-ref params arrive as `PhpRef`)
//!   reg 2 (`REG_THIS`):  `$this` PhpObject (or `null`)
//!   reg 3 (`REG_CALLED`): late-static-binding `static::` class metadata (or `null`)
//!   reg 4 (`REG_SCOPE`): caller `Map<string, PhpRef>` for top-level / `include` bodies (or `null`)

use super::resolve::NsResolver;
use super::symtable::{is_superglobal, FnScopeInfo};
use crate::bytecode::{FuncProto, Instr, Program, Reg};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use zipp_phpparse::ast::{ClosureUse, Param};

pub(super) type R<T> = Result<T, String>;

pub(super) const MAX_FUNCTIONS: usize = 32768;
pub(super) const MAX_INSTRUCTIONS: usize = 1 << 20;
pub(super) const MAX_DEPTH: usize = 128;

pub(super) const REG_FN: Reg = 0;
pub(super) const REG_ARGS: Reg = 1;
pub(super) const REG_THIS: Reg = 2;
pub(super) const REG_CALLED: Reg = 3;
pub(super) const REG_SCOPE: Reg = 4;

/// Context of one enclosing loop or switch (for PHP's `break N` and `continue N`).
pub(super) struct LoopCtx {
    pub breaks: Vec<usize>,
    pub continues: Vec<usize>,
    pub handler_depth: usize,
}

/// Source-file context for magic constants (`__FILE__`, `__DIR__`, `__LINE__`).
#[derive(Clone)]
pub(super) struct FileCtx {
    pub file: String,
    pub dir: String,
    pub lines: Vec<u32>,
    pub strict_types: bool,
}

impl FileCtx {
    pub fn new(file: &str, dir: &str, source: &str) -> Self {
        let mut lines = vec![0u32];
        for (i, &b) in source.as_bytes().iter().enumerate() {
            if b == b'\n' {
                lines.push(i as u32 + 1);
            }
        }
        Self {
            file: file.to_owned(),
            dir: dir.to_owned(),
            lines,
            strict_types: false,
        }
    }

    pub fn line_of(&self, offset: u32) -> i64 {
        self.lines.partition_point(|&start| start <= offset).max(1) as i64
    }
}

/// Storage kind of a variable inside a non-dynamic-scope function.
#[derive(Debug, Clone, Copy)]
pub(super) enum VarSlot {
    /// Register holds the PHP value directly.
    Plain(Reg),
    /// Register holds a `PhpRef` box `{ __php: 5, v }`.
    Ref(Reg),
}

/// Where compiled `FuncProto`s are appended (either directly to a `Program` or to a
/// dynamic-include `Vec<FuncProto>` with a base function ID offset).
pub(super) enum FuncSink<'a> {
    Program(&'a RefCell<Program>),
    Dynamic {
        base_id: u32,
        protos: &'a RefCell<Vec<FuncProto>>,
    },
}

impl<'a> FuncSink<'a> {
    pub fn push(&self, proto: FuncProto) -> R<u32> {
        match self {
            FuncSink::Program(p) => {
                let mut p = p.borrow_mut();
                if p.functions.len() >= MAX_FUNCTIONS {
                    return Err("PHP: function count limit exceeded".into());
                }
                let id = p.functions.len() as u32;
                p.functions.push(proto);
                Ok(id)
            }
            FuncSink::Dynamic { base_id, protos } => {
                let mut v = protos.borrow_mut();
                let id = *base_id + v.len() as u32;
                v.push(proto);
                Ok(id)
            }
        }
    }

    pub fn next_id(&self) -> u32 {
        match self {
            FuncSink::Program(p) => p.borrow().functions.len() as u32,
            FuncSink::Dynamic { base_id, protos } => *base_id + protos.borrow().len() as u32,
        }
    }

    pub fn alloc_global(&self, name: String) -> Option<u32> {
        match self {
            FuncSink::Program(p) => {
                let mut p = p.borrow_mut();
                let idx = p.global_names.len() as u32;
                p.global_names.push(name);
                Some(idx)
            }
            FuncSink::Dynamic { .. } => None,
        }
    }
}

pub(super) struct PhpEmitter<'a> {
    pub proto: FuncProto,
    pub rt_slot: u32,
    pub sink: &'a FuncSink<'a>,
    pub file: FileCtx,
    pub ns: NsResolver,
    /// Enclosing class FQN (when compiling a method or closure inside a class).
    pub class_name: Option<String>,
    /// Declared simple non-private, non-readonly, hook-free properties on `class_name`
    /// mapping `prop_name -> Option<TYPEOF_NAMES code>` (`Some(7)` for `int`, `Some(1)` for `string`,
    /// `Some(2)` for `bool`, `Some(0)` for `float`, `None` for untyped).
    pub class_fast_props: HashMap<String, Option<u8>>,
    /// Lowercase names of `private` methods declared on `class_name`.
    pub class_private_methods: std::collections::HashSet<String>,
    /// Enclosing trait FQN (for `__TRAIT__`).
    pub trait_name: Option<String>,
    /// Enclosing function/method name (for `__FUNCTION__` / `__METHOD__`).
    pub fn_name: String,

    pub next: usize,
    high: usize,

    /// Fixed register holding `__zipp_php` (`R`).
    pub r_rt: Reg,
    /// Fixed register holding the active `Map<string, PhpRef>` when `use_scope_map` is true.
    pub r_scope: Option<Reg>,
    /// Fixed register holding the resolved `callerScopeCls` class metadata (or `null`).
    pub r_scope_cls: Option<Reg>,

    /// Variable slots when `use_scope_map == false`.
    pub vars: BTreeMap<String, VarSlot>,
    pub use_scope_map: bool,

    pub loops: Vec<LoopCtx>,
    pub handler_depth: usize,
    pub labels: HashMap<String, (u32, usize)>,
    pub pending_gotos: Vec<(String, usize, usize)>,

    string_ids: HashMap<String, u32>,
    string_consts: HashMap<u32, u32>,
    float_consts: HashMap<u64, u32>,
}

impl<'a> PhpEmitter<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        rt_slot: u32,
        sink: &'a FuncSink<'a>,
        file: FileCtx,
        ns: NsResolver,
        class_name: Option<String>,
        trait_name: Option<String>,
        fn_name: String,
        scope_info: &FnScopeInfo,
        params: &[Param],
        uses: &[ClosureUse],
        is_top_level: bool,
        is_direct: bool,
    ) -> R<Self> {
        let mut proto = crate::compile::placeholder(name);
        proto.is_strict = true;
        proto.non_constructable = true;
        proto.simple_params = true;
        proto.param_count = 4; // (args, phpThis, calledCls, scopeMap)
        proto.length = 4;
        proto.is_generator = scope_info.is_generator;

        let mut e = Self {
            proto,
            rt_slot,
            sink,
            file,
            ns,
            class_name,
            class_fast_props: HashMap::new(),
            class_private_methods: std::collections::HashSet::new(),
            trait_name,
            fn_name,
            next: 5, // regs 0..=4 are ABI registers
            high: 5,
            r_rt: 0,
            r_scope: None,
            r_scope_cls: None,
            vars: BTreeMap::new(),
            use_scope_map: scope_info.use_scope_map,
            loops: Vec::new(),
            handler_depth: 0,
            labels: HashMap::new(),
            pending_gotos: Vec::new(),
            string_ids: HashMap::new(),
            string_consts: HashMap::new(),
            float_consts: HashMap::new(),
        };

        // Load `__zipp_php` into a fixed register.
        let r_rt = e.alloc()?;
        e.emit(Instr::LoadGlobal {
            dst: r_rt,
            idx: rt_slot,
        })?;
        e.r_rt = r_rt;

        // Resolve enclosing class scope metadata once:
        // - Closures read `REG_FN.scopeCls` dynamically so `Closure::bind` / `bindTo` / `call` work!
        // - Methods/traits read `REG_FN.owner` (fast single GetProp; works for trait methods too!).
        // - Default thunks fall back to static class resolution.
        if name == "{closure}" {
            let r_cls = e.prop(REG_FN, "scopeCls")?;
            e.r_scope_cls = Some(r_cls);
        } else if name == "<default>" {
            if let Some(cls) = e.class_name.clone() {
                let cname = e.string(&cls)?;
                let r_cls = e.helper("resolveCls", &[cname])?;
                e.r_scope_cls = Some(r_cls);
            }
        } else if e.class_name.is_some() || e.trait_name.is_some() {
            let r_cls = e.prop(REG_FN, "owner")?;
            e.r_scope_cls = Some(r_cls);
        }

        if scope_info.use_scope_map {
            let scope_reg = if is_top_level {
                REG_SCOPE
            } else {
                e.helper("newScope", &[])?
            };
            e.r_scope = Some(scope_reg);
            // Populate params into scope map
            for (i, p) in params.iter().enumerate() {
                let idx = e.small_int(i as i32)?;
                let val = e.alloc()?;
                e.emit(Instr::GetIndex {
                    dst: val,
                    obj: REG_ARGS,
                    key: idx,
                })?;
                let pname = e.string(&p.name)?;
                if p.by_ref {
                    e.helper("sbind", &[scope_reg, pname, val])?;
                } else {
                    e.helper("sset", &[scope_reg, pname, val])?;
                }
            }
            // Populate closure `use` captures into scope map
            if !uses.is_empty() {
                let uses_arr = e.prop(REG_FN, "uses")?;
                for (i, u) in uses.iter().enumerate() {
                    let idx = e.small_int(i as i32)?;
                    let val = e.alloc()?;
                    e.emit(Instr::GetIndex {
                        dst: val,
                        obj: uses_arr,
                        key: idx,
                    })?;
                    let uname = e.string(&u.name)?;
                    if u.by_ref {
                        e.helper("sbind", &[scope_reg, uname, val])?;
                    } else {
                        e.helper("sset", &[scope_reg, uname, val])?;
                    }
                }
            }
        } else {
            // Allocate registers for params first
            let mut allocated = std::collections::HashSet::new();
            for (i, p) in params.iter().enumerate() {
                if is_superglobal(&p.name) {
                    continue;
                }
                let val = if is_direct {
                    // Direct ABI: p0 arrives directly in reg 1, p1 directly in reg 4!
                    if i == 0 { REG_ARGS } else { REG_SCOPE }
                } else {
                    let idx = e.small_int(i as i32)?;
                    let v = e.alloc()?;
                    e.emit(Instr::GetIndex {
                        dst: v,
                        obj: REG_ARGS,
                        key: idx,
                    })?;
                    v
                };
                if scope_info.ref_vars.contains(&p.name) {
                    let cell = if p.by_ref { val } else { e.helper("ref", &[val])? };
                    e.vars.insert(p.name.clone(), VarSlot::Ref(cell));
                } else {
                    e.vars.insert(p.name.clone(), VarSlot::Plain(val));
                }
                allocated.insert(p.name.clone());
            }
            // Closure `use` captures
            if !uses.is_empty() {
                let uses_arr = e.prop(REG_FN, "uses")?;
                for (i, u) in uses.iter().enumerate() {
                    if is_superglobal(&u.name) || allocated.contains(&u.name) {
                        continue;
                    }
                    let idx = e.small_int(i as i32)?;
                    let val = e.alloc()?;
                    e.emit(Instr::GetIndex {
                        dst: val,
                        obj: uses_arr,
                        key: idx,
                    })?;
                    if scope_info.ref_vars.contains(&u.name) {
                        let cell = if u.by_ref { val } else { e.helper("ref", &[val])? };
                        e.vars.insert(u.name.clone(), VarSlot::Ref(cell));
                    } else {
                        e.vars.insert(u.name.clone(), VarSlot::Plain(val));
                    }
                    allocated.insert(u.name.clone());
                }
            }
            // Remaining locals start at PHP `null` (or `PhpRef(null)`).
            for name in &scope_info.locals {
                if allocated.contains(name) {
                    continue;
                }
                let n = e.null()?;
                if scope_info.ref_vars.contains(name) {
                    let cell = e.helper("ref", &[n])?;
                    e.vars.insert(name.clone(), VarSlot::Ref(cell));
                } else {
                    e.vars.insert(name.clone(), VarSlot::Plain(n));
                }
            }
        }

        if e.proto.is_generator {
            e.emit(Instr::GenStart)?;
        }
        Ok(e)
    }

    pub fn patch_gotos(&mut self) -> R<()> {
        for (name, jmp_pc, src_depth) in std::mem::take(&mut self.pending_gotos) {
            let Some(&(dst_pc, dst_depth)) = self.labels.get(&name) else {
                return Err(format!("PHP: 'goto' to undefined label '{name}'"));
            };
            self.proto.code[jmp_pc] = if src_depth > dst_depth {
                Instr::JumpFinally {
                    target: dst_pc,
                    floor: dst_depth as u16,
                }
            } else {
                Instr::Jump { target: dst_pc }
            };
        }
        Ok(())
    }

    pub fn finish(mut self) -> FuncProto {
        self.proto.reg_count = self.high.max(self.next) as u16;
        self.proto.code.shrink_to_fit();
        self.proto.constants.shrink_to_fit();
        self.proto.string_constants.shrink_to_fit();
        self.proto
    }

    // ---- Register management ----
    pub fn alloc(&mut self) -> R<Reg> {
        self.block(1)
    }

    pub fn block(&mut self, count: usize) -> R<Reg> {
        let count = count.max(1);
        let end = self.next.checked_add(count).ok_or("PHP: register overflow")?;
        if end >= u16::MAX as usize {
            return Err("PHP: register limit exceeded in one function".into());
        }
        let base = self.next as Reg;
        self.next = end;
        self.high = self.high.max(end);
        Ok(base)
    }

    pub fn mark(&self) -> usize {
        self.next
    }

    pub fn release(&mut self, mark: usize) {
        self.next = mark;
    }

    // ---- Instruction emission & patching ----
    #[inline(never)]
    pub fn emit(&mut self, instr: Instr) -> R<usize> {
        if self.proto.code.len() >= MAX_INSTRUCTIONS {
            return Err("PHP: per-function bytecode limit exceeded".into());
        }
        let at = self.proto.code.len();
        self.proto.code.push(instr);
        Ok(at)
    }

    pub fn here(&self) -> u32 {
        self.proto.code.len() as u32
    }

    pub fn patch(&mut self, at: usize, target: u32) -> R<()> {
        match self.proto.code.get_mut(at) {
            Some(Instr::Jump { target: dst })
            | Some(Instr::JumpIfFalse { target: dst, .. })
            | Some(Instr::JumpIfTrue { target: dst, .. })
            | Some(Instr::JumpIfNotLt { target: dst, .. })
            | Some(Instr::JumpIfNotLe { target: dst, .. })
            | Some(Instr::JumpFinally { target: dst, .. })
            | Some(Instr::PushHandler {
                catch_target: dst, ..
            })
            | Some(Instr::PushFinally { target: dst, .. }) => {
                *dst = target;
                Ok(())
            }
            _ => Err("PHP emitter: invalid jump patch".into()),
        }
    }

    pub fn jump(&mut self) -> R<usize> {
        self.emit(Instr::Jump { target: 0 })
    }

    pub fn jump_if_false(&mut self, cond: Reg) -> R<usize> {
        self.emit(Instr::JumpIfFalse { cond, target: 0 })
    }

    pub fn jump_if_true(&mut self, cond: Reg) -> R<usize> {
        self.emit(Instr::JumpIfTrue { cond, target: 0 })
    }

    // ---- Constants ----
    pub fn string_index(&mut self, value: &str) -> u32 {
        if let Some(&index) = self.string_ids.get(value) {
            return index;
        }
        let index = self.proto.string_constants.len() as u32;
        self.proto.string_constants.push(value.to_owned());
        self.string_ids.insert(value.to_owned(), index);
        index
    }

    pub fn string_const(&mut self, value: &str) -> u32 {
        let si = self.string_index(value);
        match self.string_consts.get(&si) {
            Some(&idx) => idx,
            None => {
                let idx = self.proto.constants.len() as u32;
                self.proto
                    .constants
                    .push(crate::value::Value::heap(crate::vm::STRING_CONST_BIT | si));
                self.string_consts.insert(si, idx);
                idx
            }
        }
    }

    pub fn string(&mut self, value: &str) -> R<Reg> {
        let dst = self.alloc()?;
        let idx = self.string_const(value);
        self.emit(Instr::LoadConst { dst, idx })?;
        Ok(dst)
    }

    pub fn int64(&mut self, value: i64) -> R<Reg> {
        let dst = self.alloc()?;
        let instr =
            crate::bytecode::load_bigint_instr(dst, value as i128, &mut self.proto.bigint_consts);
        self.emit(instr)?;
        Ok(dst)
    }

    pub fn small_int(&mut self, val: i32) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::LoadInt { dst, val })?;
        Ok(dst)
    }

    pub fn float(&mut self, value: f64) -> R<Reg> {
        let dst = self.alloc()?;
        let idx = match self.float_consts.get(&value.to_bits()) {
            Some(&idx) => idx,
            None => {
                let idx = self.proto.constants.len() as u32;
                self.proto.constants.push(crate::value::Value::num(value));
                self.float_consts.insert(value.to_bits(), idx);
                idx
            }
        };
        self.emit(Instr::LoadConst { dst, idx })?;
        Ok(dst)
    }

    pub fn boolean(&mut self, val: bool) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::LoadBool { dst, val })?;
        Ok(dst)
    }

    pub fn null(&mut self) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::LoadNull { dst })?;
        Ok(dst)
    }

    pub fn undefined(&mut self) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::LoadUndefined { dst })?;
        Ok(dst)
    }

    // ---- Objects, arrays, helper calls ----
    pub fn prop(&mut self, obj: Reg, key: &str) -> R<Reg> {
        let dst = self.alloc()?;
        let name = self.string_index(key);
        self.emit(Instr::GetProp { dst, obj, name })?;
        Ok(dst)
    }

    pub fn set_prop(&mut self, obj: Reg, key: &str, val: Reg) -> R<()> {
        let name = self.string_index(key);
        self.emit(Instr::SetProp {
            obj,
            name,
            val,
            strict: false,
        })?;
        Ok(())
    }

    pub fn new_js_obj(&mut self) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::NewObject { dst, hint: 0 })?;
        Ok(dst)
    }

    pub fn js_array(&mut self, values: &[Reg]) -> R<Reg> {
        let dst = self.alloc()?;
        let argc = u16::try_from(values.len()).map_err(|_| "PHP: too many array elements")?;
        if argc == 0 {
            self.emit(Instr::NewArray {
                dst,
                arg_base: 0,
                argc: 0,
            })?;
            return Ok(dst);
        }
        let base = self.block(values.len())?;
        for (i, &src) in values.iter().enumerate() {
            self.emit(Instr::Move {
                dst: base + i as u16,
                src,
            })?;
        }
        self.emit(Instr::NewArray {
            dst,
            arg_base: base,
            argc,
        })?;
        Ok(dst)
    }

    pub fn helper(&mut self, method: &str, args: &[Reg]) -> R<Reg> {
        let callee = self.prop(self.r_rt, method)?;
        let argc = u16::try_from(args.len()).map_err(|_| "PHP: too many helper arguments")?;
        let base = if argc == 0 {
            0
        } else {
            let b = self.block(args.len())?;
            for (i, &src) in args.iter().enumerate() {
                self.emit(Instr::Move {
                    dst: b + i as u16,
                    src,
                })?;
            }
            b
        };
        let dst = self.alloc()?;
        self.emit(Instr::Call {
            dst,
            callee,
            arg_base: base,
            argc,
        })?;
        Ok(dst)
    }

    /// Return register holding the enclosing class scope metadata (or `null`).
    pub fn scope_cls_reg(&mut self) -> R<Reg> {
        match self.r_scope_cls {
            Some(r) => Ok(r),
            None => self.null(),
        }
    }

    // ---- Variable access (unified across plain registers, PhpRef cells, scope maps, and superglobals) ----
    pub fn load_var(&mut self, name: &str, quiet: bool) -> R<Reg> {
        if name == "this" {
            let dst = self.alloc()?;
            self.emit(Instr::Move {
                dst,
                src: REG_THIS,
            })?;
            return Ok(dst);
        }
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            let q = self.boolean(quiet)?;
            return self.helper("sget", &[scope, n, q]);
        }
        match self.vars.get(name).copied() {
            Some(VarSlot::Plain(r)) => {
                let dst = self.alloc()?;
                self.emit(Instr::Move { dst, src: r })?;
                Ok(dst)
            }
            Some(VarSlot::Ref(cell)) => self.prop(cell, "v"),
            None => self.null(),
        }
    }

    /// Obtain the `PhpRef` box for variable `$name` (for `&$x`, by-ref calls, `foreach as &$v`).
    pub fn load_var_ref(&mut self, name: &str) -> R<Reg> {
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            return self.helper("sref", &[scope, n]);
        }
        match self.vars.get(name).copied() {
            Some(VarSlot::Ref(cell)) => Ok(cell),
            Some(VarSlot::Plain(r)) => self.helper("ref", &[r]),
            None => {
                let n = self.null()?;
                self.helper("ref", &[n])
            }
        }
    }

    /// Assign a PHP value into `$name` (preserving CoW sharing and existing reference aliases).
    pub fn store_var(&mut self, name: &str, val: Reg) -> R<Reg> {
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            return self.helper("sset", &[scope, n, val]);
        }
        match self.vars.get(name).copied() {
            Some(VarSlot::Plain(r)) => {
                let shared = self.helper("share", &[val])?;
                self.emit(Instr::Move { dst: r, src: shared })?;
                Ok(shared)
            }
            Some(VarSlot::Ref(cell)) => self.helper("rset", &[cell, val]),
            None => Ok(val),
        }
    }

    /// Assign an already-owned value (known scalar, fresh array literal, or CoW write-back)
    /// into `$name` WITHOUT calling `R.share()`, preserving `shared == false` for O(1) CoW!
    pub fn store_var_owned(&mut self, name: &str, val: Reg) -> R<Reg> {
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            return self.helper("ssetOwned", &[scope, n, val]);
        }
        match self.vars.get(name).copied() {
            Some(VarSlot::Plain(r)) => {
                self.emit(Instr::Move { dst: r, src: val })?;
                Ok(val)
            }
            Some(VarSlot::Ref(cell)) => {
                self.set_prop(cell, "v", val)?;
                Ok(val)
            }
            None => Ok(val),
        }
    }

    /// Emit `Instr::TypeOfIs` (`typeof a === TYPEOF_NAMES[code]` xor `neg`).
    pub fn type_of_is(&mut self, a: Reg, code: u8, neg: bool) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::TypeOfIs { dst, a, code, neg })?;
        Ok(dst)
    }

    /// Bind variable `$name` to alias `ref_reg` (`$a = &$b`, `global $x`, `static $x`).
    pub fn bind_var_ref(&mut self, name: &str, ref_reg: Reg) -> R<()> {
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            self.helper("sbind", &[scope, n, ref_reg])?;
            return Ok(());
        }
        if let Some(VarSlot::Ref(cell)) = self.vars.get(name).copied() {
            self.emit(Instr::Move {
                dst: cell,
                src: ref_reg,
            })?;
        }
        Ok(())
    }

    /// `unset($name)`: breaks alias or resets local to `null`.
    pub fn unset_var(&mut self, name: &str) -> R<()> {
        if is_superglobal(name) || self.use_scope_map {
            let scope = match self.r_scope {
                Some(r) => r,
                None => self.null()?,
            };
            let n = self.string(name)?;
            self.helper("sunset", &[scope, n])?;
            return Ok(());
        }
        let n = self.null()?;
        match self.vars.get(name).copied() {
            Some(VarSlot::Plain(r)) => {
                self.emit(Instr::Move { dst: r, src: n })?;
            }
            Some(VarSlot::Ref(cell)) => {
                let fresh = self.helper("ref", &[n])?;
                self.emit(Instr::Move {
                    dst: cell,
                    src: fresh,
                })?;
            }
            None => {}
        }
        Ok(())
    }

    pub fn truthy(&mut self, val: Reg) -> R<Reg> {
        self.helper("truthy", &[val])
    }
}
