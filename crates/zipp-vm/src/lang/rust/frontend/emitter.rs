//! Core Rust-to-ZIPP register bytecode emitter (uses only base `Instr` opcodes).
//!
//! ABI of every compiled Rust function / method / closure:
//!   reg 0: `this` (unused for plain `fn` / methods; closure capture env array for capturing closures)
//!   reg 1..=N: positional parameters directly in registers (enables direct `Instr::Call` and native x86-64 JIT!)

use super::scope::{
    analyze_fn, extract_axum_params, extract_derives, extract_result_err_type,
    extract_return_ok_desc, extract_serde_container, extract_serde_field, has_attr,
    item_cfg_enabled, mangle, qualify, type_base_name, type_path_str, CrateSymbols,
    FnAnalysis, NumKind,
};
use crate::bytecode::{FuncProto, Instr, Program, Reg};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use zipp_rsparse::ast::*;

pub(super) type R<T> = Result<T, String>;

pub(super) const MAX_FUNCTIONS: usize = 32768;
pub(super) const MAX_INSTRUCTIONS: usize = 1 << 20;

pub(super) struct FileCtx<'a> {
    pub file: &'a str,
    pub source: &'a str,
    pub lines: Vec<u32>,
    pub is_test_mode: bool,
    pub read_relative: &'a dyn Fn(&str) -> Result<String, String>,
}

impl<'a> FileCtx<'a> {
    pub fn new(
        file: &'a str,
        source: &'a str,
        is_test_mode: bool,
        read_relative: &'a dyn Fn(&str) -> Result<String, String>,
    ) -> Self {
        let mut lines = vec![0u32];
        for (i, &b) in source.as_bytes().iter().enumerate() {
            if b == b'\n' {
                lines.push(i as u32 + 1);
            }
        }
        Self {
            file,
            source,
            lines,
            is_test_mode,
            read_relative,
        }
    }

    pub fn line_of(&self, offset: u32) -> usize {
        self.lines.partition_point(|&start| start <= offset).max(1)
    }
}

#[derive(Clone, Copy)]
pub(super) struct LocalVar {
    pub reg: Reg,
    pub boxed: bool,
    pub num_kind: NumKind,
}

pub(super) struct LoopCtx {
    pub label: Option<String>,
    pub result_reg: Option<Reg>,
    pub breaks: Vec<usize>,
    pub continues: Vec<usize>,
}

pub(super) struct RsEmitter<'a> {
    pub proto: FuncProto,
    pub rt_slot: u32,
    pub program: &'a RefCell<Program>,
    pub syms: &'a CrateSymbols,
    pub file: &'a FileCtx<'a>,
    pub mod_prefix: String,
    pub self_type: Option<String>,
    /// Error type name `E` of the enclosing function if it returns `Result<_, E>` (for `?` `From` conversion).
    pub ret_err_type: Option<String>,

    pub next: usize,
    high: usize,

    /// Fixed register holding `__zipp_rs`.
    pub r_rt: Reg,

    /// Lexical scopes (innermost last), supporting Rust variable shadowing.
    pub scopes: Vec<HashMap<String, LocalVar>>,
    /// Variable names in THIS function that should be stored in a `{ __rs: 6, v }` ref cell.
    pub boxed_vars: HashSet<String>,
    /// Enclosing loops for `break` / `continue` (labeled & unlabeled, `break val`).
    pub loops: Vec<LoopCtx>,
    /// Local `use` aliases declared inside blocks.
    pub local_uses: HashMap<String, Vec<String>>,

    string_ids: HashMap<String, u32>,
    string_consts: HashMap<u32, u32>,
    float_consts: HashMap<u64, u32>,
}

impl<'a> RsEmitter<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        param_count: u16,
        is_async: bool,
        rt_slot: u32,
        program: &'a RefCell<Program>,
        syms: &'a CrateSymbols,
        file: &'a FileCtx<'a>,
        mod_prefix: String,
        self_type: Option<String>,
        boxed_vars: HashSet<String>,
    ) -> R<Self> {
        let mut proto = crate::compile::placeholder(name);
        proto.is_strict = true;
        proto.non_constructable = true;
        proto.simple_params = true;
        proto.param_count = param_count;
        proto.length = param_count;
        proto.is_async = is_async;

        let start_reg = 1 + param_count as usize;
        let mut e = Self {
            proto,
            rt_slot,
            program,
            syms,
            file,
            mod_prefix,
            self_type,
            ret_err_type: None,
            next: start_reg,
            high: start_reg,
            r_rt: 0,
            scopes: vec![HashMap::new()],
            boxed_vars,
            loops: Vec::new(),
            local_uses: HashMap::new(),
            string_ids: HashMap::new(),
            string_consts: HashMap::new(),
            float_consts: HashMap::new(),
        };

        let r_rt = e.alloc()?;
        e.emit(Instr::LoadGlobal {
            dst: r_rt,
            idx: rt_slot,
        })?;
        e.r_rt = r_rt;
        Ok(e)
    }

    pub fn finish(mut self) -> FuncProto {
        self.proto.reg_count = self.high.max(self.next) as u16;
        self.proto.code.shrink_to_fit();
        self.proto.constants.shrink_to_fit();
        self.proto.string_constants.shrink_to_fit();
        self.proto
    }

    pub fn push_func(&self, proto: FuncProto) -> R<u32> {
        let mut p = self.program.borrow_mut();
        if p.functions.len() >= MAX_FUNCTIONS {
            return Err("Rust: function count limit exceeded".into());
        }
        let id = p.functions.len() as u32;
        p.functions.push(proto);
        Ok(id)
    }

    // ---- Register management ----

    pub fn alloc(&mut self) -> R<Reg> {
        self.block(1)
    }

    pub fn block(&mut self, count: usize) -> R<Reg> {
        let count = count.max(1);
        let end = self.next.checked_add(count).ok_or("Rust: register overflow")?;
        if end >= u16::MAX as usize {
            return Err("Rust: register limit exceeded in one function".into());
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

    // ---- Instruction emission & jump patching ----

    pub fn emit(&mut self, instr: Instr) -> R<usize> {
        if self.proto.code.len() >= MAX_INSTRUCTIONS {
            return Err("Rust: per-function bytecode limit exceeded".into());
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
            | Some(Instr::JumpIfNotLe { target: dst, .. }) => {
                *dst = target;
                Ok(())
            }
            _ => Err("Rust emitter: invalid jump patch".into()),
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

    // ---- Constant loading ----

    pub fn string_index(&mut self, value: &str) -> u32 {
        if let Some(&idx) = self.string_ids.get(value) {
            return idx;
        }
        let idx = self.proto.string_constants.len() as u32;
        self.proto.string_constants.push(value.to_owned());
        self.string_ids.insert(value.to_owned(), idx);
        idx
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

    pub fn small_int(&mut self, val: i32) -> R<Reg> {
        let dst = self.alloc()?;
        self.emit(Instr::LoadInt { dst, val })?;
        Ok(dst)
    }

    pub fn bigint(&mut self, val: i128) -> R<Reg> {
        let dst = self.alloc()?;
        let instr = crate::bytecode::load_bigint_instr(dst, val, &mut self.proto.bigint_consts);
        self.emit(instr)?;
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
        let argc = u16::try_from(values.len()).map_err(|_| "Rust: too many array elements")?;
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
        self.call_raw(callee, args)
    }

    pub fn call_raw(&mut self, callee: Reg, args: &[Reg]) -> R<Reg> {
        let argc = u16::try_from(args.len()).map_err(|_| "Rust: too many call arguments")?;
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

    // ---- Scopes & Variables ----

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn lookup_local(&self, name: &str) -> Option<LocalVar> {
        for sc in self.scopes.iter().rev() {
            if let Some(&lv) = sc.get(name) {
                return Some(lv);
            }
        }
        None
    }

    pub fn bind_local(&mut self, name: &str, val_reg: Reg, num_kind: NumKind) -> R<Reg> {
        let should_box = self.boxed_vars.contains(name);
        let slot_reg = if should_box {
            self.helper("mkRef", &[val_reg])?
        } else {
            let r = self.alloc()?;
            self.emit(Instr::Move { dst: r, src: val_reg })?;
            r
        };
        self.scopes.last_mut().unwrap().insert(
            name.to_owned(),
            LocalVar {
                reg: slot_reg,
                boxed: should_box,
                num_kind,
            },
        );
        Ok(slot_reg)
    }

    pub fn bind_existing_cell(&mut self, name: &str, cell_reg: Reg, num_kind: NumKind) {
        self.scopes.last_mut().unwrap().insert(
            name.to_owned(),
            LocalVar {
                reg: cell_reg,
                boxed: true,
                num_kind,
            },
        );
    }

    pub fn load_local(&mut self, lv: LocalVar) -> R<Reg> {
        if lv.boxed {
            self.prop(lv.reg, "v")
        } else {
            let dst = self.alloc()?;
            self.emit(Instr::Move {
                dst,
                src: lv.reg,
            })?;
            Ok(dst)
        }
    }

    pub fn store_local(&mut self, lv: LocalVar, val: Reg) -> R<()> {
        if lv.boxed {
            self.set_prop(lv.reg, "v", val)
        } else {
            self.emit(Instr::Move {
                dst: lv.reg,
                src: val,
            })?;
            Ok(())
        }
    }

    pub fn resolve_segments(&self, raw: &[String]) -> Vec<String> {
        if let Some(first) = raw.first() {
            if first == "Self" {
                if let Some(ref st) = self.self_type {
                    let mut out = vec![st.clone()];
                    out.extend_from_slice(&raw[1..]);
                    return out;
                }
            }
            if let Some(target) = self.local_uses.get(first) {
                let mut out = target.clone();
                out.extend_from_slice(&raw[1..]);
                return out;
            }
        }
        self.syms.resolve_path(&self.mod_prefix, raw)
    }

    // ---- Top-level / module item compilation ----

    pub fn emit_items(&mut self, items: &[Item]) -> R<()> {
        // Deferred `#[serde(default = "fn_name")]` bindings wired in Pass 2 after functions exist.
        let mut deferred_serde_defaults: Vec<(String, String, u32)> = Vec::new();
        // Pass 1: register struct & enum metadata, trait defaults, and compile all functions / impl methods
        // so forward calls work regardless of declaration order!
        for item in items {
            if !item_cfg_enabled(&item.attrs, self.file.is_test_mode) {
                continue;
            }
            let derives = extract_derives(&item.attrs);
            match &item.kind {
                ItemKind::Struct(s) => {
                    let sqname = qualify(&self.mod_prefix, &s.name);
                    let sname = self.string(&sqname)?;
                    let cmeta = extract_serde_container(&item.attrs);
                    let serde_obj = self.new_js_obj()?;
                    if let Some(ref ra) = cmeta.rename_all {
                        let r = self.string(ra)?;
                        self.set_prop(serde_obj, "rename_all", r)?;
                    }
                    let fields_meta_obj = self.new_js_obj()?;
                    let (is_tuple, field_regs) = match &s.fields {
                        StructFields::Named(fs) => {
                            let mut pairs = Vec::with_capacity(fs.len());
                            for f in fs {
                                let fn_r = self.string(&f.name)?;
                                let ft_r = self.string(
                                    &self.syms.type_desc_resolved(&self.mod_prefix, &f.ty),
                                )?;
                                pairs.push(self.js_array(&[fn_r, ft_r])?);
                                let fm = extract_serde_field(&f.attrs);
                                if fm.rename.is_some()
                                    || fm.alias.is_some()
                                    || fm.skip
                                    || fm.skip_serializing
                                    || fm.skip_deserializing
                                    || fm.skip_serializing_if.is_some()
                                    || fm.flatten
                                    || fm.has_default
                                {
                                    let fobj = self.new_js_obj()?;
                                    if let Some(ref rn) = fm.rename {
                                        let v = self.string(rn)?;
                                        self.set_prop(fobj, "rename", v)?;
                                    }
                                    if let Some(ref al) = fm.alias {
                                        let v = self.string(al)?;
                                        self.set_prop(fobj, "alias", v)?;
                                    }
                                    if fm.skip {
                                        let v = self.boolean(true)?;
                                        self.set_prop(fobj, "skip", v)?;
                                    }
                                    if fm.skip_serializing {
                                        let v = self.boolean(true)?;
                                        self.set_prop(fobj, "skip_serializing", v)?;
                                    }
                                    if fm.skip_deserializing {
                                        let v = self.boolean(true)?;
                                        self.set_prop(fobj, "skip_deserializing", v)?;
                                    }
                                    if let Some(ref sif) = fm.skip_serializing_if {
                                        let v = self.string(sif)?;
                                        self.set_prop(fobj, "skip_serializing_if", v)?;
                                    }
                                    if fm.flatten {
                                        let v = self.boolean(true)?;
                                        self.set_prop(fobj, "flatten", v)?;
                                    }
                                    if fm.has_default {
                                        let v = self.boolean(true)?;
                                        self.set_prop(fobj, "has_default", v)?;
                                    }
                                    if let Some(ref df) = fm.default_fn {
                                        let raw: Vec<String> =
                                            df.split("::").map(|s| s.trim().to_owned()).collect();
                                        if let Some((_, slot)) =
                                            self.syms.lookup_fn_global(&self.mod_prefix, &raw)
                                        {
                                            deferred_serde_defaults.push((
                                                sqname.clone(),
                                                f.name.clone(),
                                                slot,
                                            ));
                                        }
                                    }
                                    self.set_prop(fields_meta_obj, &f.name, fobj)?;
                                }
                            }
                            (false, self.js_array(&pairs)?)
                        }
                        StructFields::Tuple(fs) => {
                            let mut pairs = Vec::with_capacity(fs.len());
                            for (i, f) in fs.iter().enumerate() {
                                let fn_r = self.string(&i.to_string())?;
                                let ft_r = self.string(
                                    &self.syms.type_desc_resolved(&self.mod_prefix, &f.ty),
                                )?;
                                pairs.push(self.js_array(&[fn_r, ft_r])?);
                            }
                            (true, self.js_array(&pairs)?)
                        }
                        StructFields::Unit => (false, self.js_array(&[])?),
                    };
                    self.set_prop(serde_obj, "fields", fields_meta_obj)?;
                    let t_r = self.boolean(is_tuple)?;
                    let mut d_regs = Vec::with_capacity(derives.len());
                    for d in &derives {
                        d_regs.push(self.string(d)?);
                    }
                    let d_arr = self.js_array(&d_regs)?;
                    self.helper("regStruct", &[sname, field_regs, t_r, d_arr, serde_obj])?;
                }
                ItemKind::Enum(e) => {
                    let ename = self.string(&e.name)?;
                    let cmeta = extract_serde_container(&item.attrs);
                    let serde_obj = self.new_js_obj()?;
                    if let Some(ref ra) = cmeta.rename_all {
                        let r = self.string(ra)?;
                        self.set_prop(serde_obj, "rename_all", r)?;
                    }
                    let vobj = self.new_js_obj()?;
                    let mut next_disc: i64 = 0;
                    for v in &e.variants {
                        let d_reg = if let Some(ref de) = v.discriminant {
                            let r = self.emit_expr(de)?;
                            next_disc += 1;
                            r
                        } else {
                            let r = self.small_int(next_disc as i32)?;
                            next_disc += 1;
                            r
                        };
                        self.set_prop(vobj, &v.name, d_reg)?;
                    }
                    let mut d_regs = Vec::with_capacity(derives.len());
                    for d in &derives {
                        d_regs.push(self.string(d)?);
                    }
                    let d_arr = self.js_array(&d_regs)?;
                    self.helper("regEnum", &[ename, vobj, d_arr, serde_obj])?;
                    // `thiserror` `#[derive(Error)]` `#[from]` automatic `From<Src> for Enum` registration!
                    for v in &e.variants {
                        if let StructFields::Tuple(tfs) = &v.fields {
                            if tfs.len() == 1 && has_attr(&tfs[0].attrs, "from") {
                                let src_path = type_path_str(&tfs[0].ty);
                                let vn = self.string(&v.name)?;
                                let sp = self.string(&src_path)?;
                                self.helper("regEnumFrom", &[ename, vn, sp])?;
                            }
                        }
                    }
                }
                ItemKind::Trait(tr) => {
                    for ti in &tr.items {
                        if let ItemKind::Fn(f) = &ti.kind {
                            if f.body.is_some() {
                                let func_id = self.compile_fn_item(
                                    f,
                                    &format!("__rs_fn_{}_{}", tr.name, f.name),
                                    None,
                                )?;
                                let fn_reg = self.alloc()?;
                                self.emit(Instr::MakeFunc {
                                    dst: fn_reg,
                                    func_id,
                                })?;
                                let tn = self.string(&tr.name)?;
                                let mn = self.string(&f.name)?;
                                let mut_self = self.boolean(
                                    f.params.first().is_some_and(|p| p.is_self && p.is_mut_self),
                                )?;
                                self.helper("regTraitDefault", &[tn, mn, fn_reg, mut_self])?;
                            }
                        }
                    }
                }
                ItemKind::Fn(f) => {
                    if f.body.is_none() {
                        continue;
                    }
                    let qname = qualify(&self.mod_prefix, &f.name);
                    let proto_name = format!("__rs_fn_{}", mangle(&qname));
                    let func_id = self.compile_fn_item(f, &proto_name, None)?;
                    let fn_reg = self.alloc()?;
                    self.emit(Instr::MakeFunc {
                        dst: fn_reg,
                        func_id,
                    })?;
                    if let Some(ax_params) = extract_axum_params(self.syms, &self.mod_prefix, &f.params) {
                        let mut p_regs = Vec::with_capacity(ax_params.len());
                        for (kind, inner) in &ax_params {
                            let kr = self.string(kind)?;
                            let ir = self.string(inner)?;
                            p_regs.push(self.js_array(&[kr, ir])?);
                        }
                        let parr = self.js_array(&p_regs)?;
                        self.set_prop(fn_reg, "__rs_params", parr)?;
                    }
                    if let Some(&slot) = self.syms.fn_globals.get(&qname) {
                        self.emit(Instr::StoreGlobal {
                            idx: slot,
                            src: fn_reg,
                        })?;
                    } else {
                        // Local `fn` declared inside a block: bind in local scope!
                        self.bind_local(&f.name, fn_reg, NumKind::Unknown)?;
                    }
                    // Register `main`, `router`, and `#[test]` / `#[tokio::test]` functions
                    if self.mod_prefix.is_empty() && f.name == "main" {
                        self.helper("setMain", &[fn_reg])?;
                    }
                    if self.mod_prefix.is_empty() && f.name == "router" {
                        let k = self.string("__rs_fn_router")?;
                        self.helper("regConst", &[k, fn_reg])?;
                    }
                    if has_attr(&item.attrs, "test") {
                        let tname = self.string(&qname)?;
                        let sp = self.boolean(has_attr(&item.attrs, "should_panic"))?;
                        let ign = self.boolean(has_attr(&item.attrs, "ignore"))?;
                        self.helper("regTest", &[tname, fn_reg, sp, ign])?;
                    }
                }
                ItemKind::Impl(imp) => {
                    let self_ty_name = type_base_name(&imp.self_type);
                    let trait_seg = imp.trait_path.as_ref().and_then(|p| p.segments.last());
                    let trait_name = trait_seg.map(|s| s.ident.clone());
                    let trait_src_ty = trait_seg
                        .and_then(|s| s.type_args.first())
                        .map(type_base_name);
                    if let Some(ref tr) = trait_name {
                        let tn = self.string(tr)?;
                        let st = self.string(&self_ty_name)?;
                        self.helper("regTraitImplEmpty", &[tn, st])?;
                    }
                    for ii in &imp.items {
                        match &ii.kind {
                            ItemKind::Fn(f) if f.body.is_some() => {
                                let proto_name =
                                    format!("__rs_fn_{}_{}", mangle(&self_ty_name), f.name);
                                let func_id = self.compile_fn_item(
                                    f,
                                    &proto_name,
                                    Some(self_ty_name.clone()),
                                )?;
                                let fn_reg = self.alloc()?;
                                self.emit(Instr::MakeFunc {
                                    dst: fn_reg,
                                    func_id,
                                })?;
                                let st = self.string(&self_ty_name)?;
                                let mn = self.string(&f.name)?;
                                let mut_self = self.boolean(
                                    f.params.first().is_some_and(|p| p.is_self && p.is_mut_self),
                                )?;
                                if let Some(ref tr) = trait_name {
                                    if tr == "From" && f.name == "from" {
                                        let src = self.string(
                                            trait_src_ty.as_deref().unwrap_or(""),
                                        )?;
                                        self.helper("regFrom", &[st, src, fn_reg])?;
                                    } else if tr == "TryFrom" && f.name == "try_from" {
                                        let src = self.string(
                                            trait_src_ty.as_deref().unwrap_or(""),
                                        )?;
                                        self.helper("regTryFrom", &[st, src, fn_reg])?;
                                    } else {
                                        let tn = self.string(tr)?;
                                        self.helper(
                                            "regTraitImpl",
                                            &[tn, st, mn, fn_reg, mut_self],
                                        )?;
                                    }
                                } else {
                                    self.helper("regMethod", &[st, mn, fn_reg, mut_self])?;
                                }
                            }
                            ItemKind::Const(c) => {
                                if let Some(ref val_expr) = c.value {
                                    let saved_self = self.self_type.replace(self_ty_name.clone());
                                    let vreg = self.emit_expr(val_expr)?;
                                    self.self_type = saved_self;
                                    let k = self.string(&format!("{self_ty_name}::{}", c.name))?;
                                    self.helper("regConst", &[k, vreg])?;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                ItemKind::Mod(m) => {
                    if let Some(sub_items) = &m.items {
                        let next_mod = qualify(&self.mod_prefix, &m.name);
                        let saved_mod = std::mem::replace(&mut self.mod_prefix, next_mod);
                        self.emit_items(sub_items)?;
                        self.mod_prefix = saved_mod;
                    }
                }
                ItemKind::Use(tree) => {
                    self.record_local_use(&[], tree);
                }
                _ => {}
            }
        }

        // Pass 2: wire deferred `#[serde(default = "fn_name")]` callbacks and evaluate `const` / `static` initializers
        // (after all functions & structs exist!).
        for (sname, fname, slot) in deferred_serde_defaults {
            let fn_r = self.alloc()?;
            self.emit(Instr::LoadGlobal {
                dst: fn_r,
                idx: slot,
            })?;
            let sn = self.string(&sname)?;
            let fn_name = self.string(&fname)?;
            self.helper("setSerdeDefaultFn", &[sn, fn_name, fn_r])?;
        }
        for item in items {
            if !item_cfg_enabled(&item.attrs, self.file.is_test_mode) {
                continue;
            }
            match &item.kind {
                ItemKind::Const(c) => {
                    if let Some(ref val_expr) = c.value {
                        let vreg = self.emit_expr(val_expr)?;
                        let qname = qualify(&self.mod_prefix, &c.name);
                        if let Some(&slot) = self.syms.static_globals.get(&qname) {
                            self.emit(Instr::StoreGlobal {
                                idx: slot,
                                src: vreg,
                            })?;
                        } else {
                            self.bind_local(&c.name, vreg, NumKind::from_type(&c.ty))?;
                        }
                        let k = self.string(&qname)?;
                        self.helper("regConst", &[k, vreg])?;
                    }
                }
                ItemKind::Static(s) => {
                    let qname = qualify(&self.mod_prefix, &s.name);
                    if let Some(&slot) = self.syms.static_globals.get(&qname) {
                        let vreg = self.emit_expr(&s.value)?;
                        self.emit(Instr::StoreGlobal {
                            idx: slot,
                            src: vreg,
                        })?;
                    } else {
                        // Function-local `static`: initialize once and persist across calls!
                        let skey = format!("{}::{}@{}", self.mod_prefix, s.name, item.span.start);
                        let k = self.string(&skey)?;
                        let has = self.helper("hasStatic", &[k])?;
                        let j_init = self.jump_if_false(has)?;
                        let existing = self.helper("getStatic", &[k])?;
                        let dst = self.alloc()?;
                        self.emit(Instr::Move {
                            dst,
                            src: existing,
                        })?;
                        let j_done = self.jump()?;
                        self.patch(j_init, self.here())?;
                        let init_v = self.emit_expr(&s.value)?;
                        self.helper("setStatic", &[k, init_v])?;
                        self.emit(Instr::Move { dst, src: init_v })?;
                        self.patch(j_done, self.here())?;
                        self.bind_local(&s.name, dst, NumKind::from_type(&s.ty))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn record_local_use(&mut self, base: &[String], tree: &UseTree) {
        let mut full = base.to_vec();
        full.extend(tree.prefix.iter().cloned());
        match &tree.kind {
            UseTreeKind::Simple(alias) => {
                if let Some(name) = alias.clone().or_else(|| full.last().cloned()) {
                    if name != "self" {
                        self.local_uses.insert(name, full);
                    }
                }
            }
            UseTreeKind::Group(children) => {
                for ch in children {
                    self.record_local_use(&full, ch);
                }
            }
            UseTreeKind::Glob => {}
        }
    }

    pub fn compile_fn_item(
        &mut self,
        f: &FnItem,
        proto_name: &str,
        self_type: Option<String>,
    ) -> R<u32> {
        let body = f.body.as_ref().ok_or("function has no body")?;
        let pats: Vec<Pat> = f.params.iter().map(|p| p.pat.clone()).collect();
        let FnAnalysis { boxed_vars, .. } = analyze_fn(&pats, body, &self.syms.mut_self_methods);
        let param_count =
            u16::try_from(f.params.len()).map_err(|_| "Rust: too many parameters")?;
        let mut sub = RsEmitter::new(
            proto_name,
            param_count,
            f.is_async,
            self.rt_slot,
            self.program,
            self.syms,
            self.file,
            self.mod_prefix.clone(),
            self_type,
            boxed_vars,
        )?;
        sub.ret_err_type = extract_result_err_type(f.ret_type.as_ref());
        // Bind positional parameters (`reg 1..=N`)
        for (i, param) in f.params.iter().enumerate() {
            let preg = (1 + i) as Reg;
            let nk = param
                .ty
                .as_ref()
                .map(NumKind::from_type)
                .unwrap_or(NumKind::Unknown);
            // If param is simple `Ident` and not boxed (or `&mut self` arriving already as a ref cell!),
            // map it directly to `preg` with ZERO Move!
            if let PatKind::Ident {
                name, subpat: None, ..
            } = &param.pat.kind
            {
                if param.is_self && param.is_mut_self {
                    // `&mut self` arrives as a ref cell when called on a boxed receiver, or plain object
                    sub.scopes.last_mut().unwrap().insert(
                        name.clone(),
                        LocalVar {
                            reg: preg,
                            boxed: false,
                            num_kind: nk,
                        },
                    );
                    continue;
                }
                if !sub.boxed_vars.contains(name) {
                    sub.scopes.last_mut().unwrap().insert(
                        name.clone(),
                        LocalVar {
                            reg: preg,
                            boxed: false,
                            num_kind: nk,
                        },
                    );
                    continue;
                }
            }
            sub.emit_pat_bind(&param.pat, preg, nk)?;
        }
        let ret_ok_hint = extract_return_ok_desc(f.ret_type.as_ref());
        let ret = sub.emit_block_with_hint(body, ret_ok_hint.as_deref())?;
        sub.emit(Instr::Return { src: ret })?;
        let proto = sub.finish();
        self.push_func(proto)
    }
}
