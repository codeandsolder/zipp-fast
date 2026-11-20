//! Expression and statement bytecode compilation for the Rust frontend.

use super::emitter::{LocalVar, LoopCtx, RsEmitter, R};
use super::scope::{
    analyze_closure, analyze_fn, literal_num_kind, type_base_name, NumKind, VariantKind,
};
use crate::bytecode::{BitwiseOp, Instr, Reg};
use zipp_rsparse::ast::*;
use zipp_rsparse::token::IntSuffix;

impl<'a> RsEmitter<'a> {
    pub(super) fn emit_block(&mut self, block: &Block) -> R<Reg> {
        self.emit_block_with_hint(block, None)
    }

    pub(super) fn emit_block_with_hint(&mut self, block: &Block, hint: Option<&str>) -> R<Reg> {
        self.push_scope();
        let saved_uses = self.local_uses.clone();
        let last_enabled_idx = block
            .stmts
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| super::scope::item_cfg_enabled(&s.attrs, self.file.is_test_mode))
            .map(|(i, _)| i);
        let mut tail_reg: Option<Reg> = None;
        for (i, stmt) in block.stmts.iter().enumerate() {
            if !super::scope::item_cfg_enabled(&stmt.attrs, self.file.is_test_mode) {
                continue;
            }
            if block.tail.is_none() && Some(i) == last_enabled_idx {
                if let StmtKind::Expr {
                    expr,
                    has_semi: false,
                } = &stmt.kind
                {
                    tail_reg = Some(self.emit_expr_with_hint(expr, hint)?);
                    continue;
                }
            }
            self.emit_stmt(stmt)?;
        }
        let result = if let Some(ref tail) = block.tail {
            self.emit_expr_with_hint(tail, hint)?
        } else if let Some(r) = tail_reg {
            r
        } else {
            self.undefined()?
        };
        self.local_uses = saved_uses;
        self.pop_scope();
        Ok(result)
    }

    pub(super) fn emit_stmt(&mut self, stmt: &Stmt) -> R<()> {
        if !super::scope::item_cfg_enabled(&stmt.attrs, self.file.is_test_mode) {
            return Ok(());
        }
        let mark = self.mark();
        match &stmt.kind {
            StmtKind::Empty => {}
            StmtKind::Item(item) => {
                self.emit_items(std::slice::from_ref(item.as_ref()))?;
                // Do NOT release registers allocated for local `fn` / `const` bindings!
                return Ok(());
            }
            StmtKind::Let {
                pat,
                ty,
                init,
                else_block,
            } => {
                let nk = ty
                    .as_ref()
                    .map(NumKind::from_type)
                    .filter(|k| k.is_numeric())
                    .or_else(|| init.as_ref().map(|e| self.infer_num_kind(e)))
                    .unwrap_or(NumKind::Unknown);
                let hint = ty
                    .as_ref()
                    .map(|t| self.syms.type_desc_resolved(&self.mod_prefix, t));
                let val = if let Some(ref expr) = init {
                    self.emit_expr_with_hint(expr, hint.as_deref())?
                } else {
                    self.undefined()?
                };
                if let Some(ref eb) = else_block {
                    let mut fails = Vec::new();
                    self.emit_pat_match(pat, val, &mut fails)?;
                    let j_done = self.jump()?;
                    let fail_pc = self.here();
                    for fj in fails {
                        self.patch(fj, fail_pc)?;
                    }
                    let _ = self.emit_block(eb)?;
                    self.patch(j_done, self.here())?;
                } else {
                    self.emit_pat_bind(pat, val, nk)?;
                }
                // Do NOT release registers allocated for new local bindings!
                return Ok(());
            }
            StmtKind::Expr { expr, .. } => {
                let _ = self.emit_expr(expr)?;
            }
        }
        self.release(mark);
        Ok(())
    }

    /// Infer whether an expression is guaranteed to evaluate to an unboxed JS Number
    /// (Int vs Float vs Unknown), enabling direct ZIPP VM arithmetic/comparison opcodes
    /// and native x86-64 Tier-A / Tier-C JIT execution!
    pub(super) fn infer_num_kind(&self, expr: &Expr) -> NumKind {
        match &expr.kind {
            ExprKind::Literal(lit) => literal_num_kind(lit),
            ExprKind::Path(p) => {
                if p.segments.len() == 1 && !p.leading_colon {
                    if let Some(lv) = self.lookup_local(&p.segments[0].ident) {
                        return lv.num_kind;
                    }
                }
                let raw: Vec<String> = p.segments.iter().map(|s| s.ident.clone()).collect();
                if let Some((q, _)) = self.syms.lookup_static_global(&self.mod_prefix, &raw) {
                    if let Some(&k) = self.syms.const_kinds.get(&q) {
                        return k;
                    }
                }
                NumKind::Unknown
            }
            ExprKind::Cast { ty, .. } => NumKind::from_type(ty),
            ExprKind::Unary {
                op: UnOp::Neg,
                expr: inner,
            } => self.infer_num_kind(inner),
            ExprKind::Binary { op, left, right } => match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Rem
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr => {
                    let lk = self.infer_num_kind(left);
                    let rk = self.infer_num_kind(right);
                    match (lk, rk) {
                        (NumKind::Float, _) | (_, NumKind::Float) => NumKind::Float,
                        (NumKind::Int, NumKind::Int) => NumKind::Int,
                        _ => NumKind::Unknown,
                    }
                }
                _ => NumKind::Unknown,
            },
            ExprKind::Call { callee, .. } => {
                if let ExprKind::Path(p) = &callee.kind {
                    let raw: Vec<String> = p.segments.iter().map(|s| s.ident.clone()).collect();
                    if let Some((q, _)) = self.syms.lookup_fn_global(&self.mod_prefix, &raw) {
                        if let Some(&k) = self.syms.fn_ret_kinds.get(&q) {
                            return k;
                        }
                    }
                }
                NumKind::Unknown
            }
            ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => b
                .tail
                .as_ref()
                .map(|t| self.infer_num_kind(t))
                .unwrap_or(NumKind::Unknown),
            ExprKind::If {
                then_block,
                else_branch: Some(eb),
                ..
            } => {
                let a = then_block
                    .tail
                    .as_ref()
                    .map(|t| self.infer_num_kind(t))
                    .unwrap_or(NumKind::Unknown);
                let b = self.infer_num_kind(eb);
                if a == b {
                    a
                } else {
                    NumKind::Unknown
                }
            }
            _ => NumKind::Unknown,
        }
    }

    pub(super) fn emit_literal(&mut self, lit: &Literal) -> R<Reg> {
        match lit {
            Literal::Int(v, suf) => {
                if matches!(suf, IntSuffix::I128 | IntSuffix::U128) || *v > 9_007_199_254_740_991 {
                    self.bigint(*v as i128)
                } else if *v <= i32::MAX as u128 {
                    self.small_int(*v as i32)
                } else {
                    self.float(*v as f64)
                }
            }
            Literal::Float(v, _) => self.float(*v),
            Literal::Bool(b) => self.boolean(*b),
            Literal::Char(c) => {
                let cp = self.small_int(*c as i32)?;
                self.helper("mkChar", &[cp])
            }
            Literal::Byte(b) => self.small_int(*b as i32),
            Literal::Str(s) => self.string(s),
            Literal::ByteStr(bs) => {
                let mut regs = Vec::with_capacity(bs.len());
                for &b in bs {
                    regs.push(self.small_int(b as i32)?);
                }
                let arr = self.js_array(&regs)?;
                self.helper("mkVec", &[arr])
            }
        }
    }

    /// Emit an expression with an optional statically-known target type descriptor
    /// (from `let x: Target = ...;`, struct literal fields, or function return type),
    /// propagating type targets into `.into()`, `.unwrap_or_default()`, `Default::default()`,
    /// `.parse()`, `.collect()`, `serde_json::from_str()`, `sqlx::query_as().fetch_*()`,
    /// and through `?`, `.await`, `.unwrap*()`, `match`, and `if` expressions!
    pub(super) fn emit_expr_with_hint(&mut self, expr: &Expr, hint: Option<&str>) -> R<Reg> {
        if let Some(target) = hint {
            if !target.is_empty() {
                let base_target = target.split('<').next().unwrap_or(target).trim();
                match &expr.kind {
                    ExprKind::Try(inner) => {
                        let v = self.emit_expr_with_hint(inner, Some(target))?;
                        let ok = self.helper("tryOk", &[v])?;
                        let j_ok = self.jump_if_true(ok)?;
                        let err_val = if let Some(ref et) = self.ret_err_type.clone() {
                            let et_r = self.string(et)?;
                            self.helper("tryErr", &[v, et_r])?
                        } else {
                            self.helper("tryErr", &[v])?
                        };
                        self.emit(Instr::Return { src: err_val })?;
                        self.patch(j_ok, self.here())?;
                        return self.helper("tryUnwrap", &[v]);
                    }
                    ExprKind::Await(inner) => {
                        let fut = self.emit_expr_with_hint(inner, Some(target))?;
                        if self.proto.is_async {
                            let dst = self.alloc()?;
                            self.emit(Instr::Await { dst, val: fut })?;
                            return Ok(dst);
                        }
                        return Ok(fut);
                    }
                    ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => {
                        return self.emit_block_with_hint(b, Some(target));
                    }
                    ExprKind::Match { scrutinee, arms } => {
                        // If a match unwraps `Ok(v)` / `Some(v)`, forward hint to the scrutinee too
                        // (e.g. `let rows: Vec<(i64, String)> = match sqlx::query_as(...).fetch_all(...).await { Ok(r) => r, ... }`)
                        let unwraps_res = arms.iter().any(|a| {
                            matches!(&a.pat.kind, PatKind::TupleStruct { path, .. }
                                if matches!(path.last_ident(), "Ok" | "Some"))
                        });
                        let val = if unwraps_res {
                            self.emit_expr_with_hint(scrutinee, Some(target))?
                        } else {
                            self.emit_expr(scrutinee)?
                        };
                        let dst = self.undefined()?;
                        let mut end_jumps = Vec::with_capacity(arms.len());
                        for arm in arms {
                            self.push_scope();
                            let mut fails = Vec::new();
                            self.emit_pat_match(&arm.pat, val, &mut fails)?;
                            if let Some(ref guard) = arm.guard {
                                let g = self.emit_expr(guard)?;
                                fails.push(self.jump_if_false(g)?);
                            }
                            let body_val = self.emit_expr_with_hint(&arm.body, Some(target))?;
                            self.emit(Instr::Move { dst, src: body_val })?;
                            self.pop_scope();
                            end_jumps.push(self.jump()?);
                            let next_pc = self.here();
                            for fj in fails {
                                self.patch(fj, next_pc)?;
                            }
                        }
                        let end_pc = self.here();
                        for ej in end_jumps {
                            self.patch(ej, end_pc)?;
                        }
                        return Ok(dst);
                    }
                    ExprKind::MethodCall {
                        receiver,
                        method,
                        type_args,
                        args,
                    } => {
                        if type_args.is_empty() && args.is_empty() && method == "into" {
                            let recv = self.emit_expr(receiver)?;
                            let dst_t = self.string(base_target)?;
                            return self.helper("convertInto", &[recv, dst_t]);
                        }
                        if matches!(
                            method.as_str(),
                            "unwrap"
                                | "expect"
                                | "unwrap_or"
                                | "unwrap_or_else"
                                | "unwrap_or_default"
                                | "ok"
                        ) {
                            let recv = self.emit_expr_with_hint(receiver, Some(target))?;
                            let mname = self.string(method)?;
                            let mut a_regs = Vec::with_capacity(args.len());
                            for a in args {
                                a_regs.push(self.emit_expr(a)?);
                            }
                            let a_arr = self.js_array(&a_regs)?;
                            if method == "unwrap_or_default" {
                                let t_reg = self.string(target)?;
                                let t_arr = self.js_array(&[t_reg])?;
                                return self.helper("callMethod", &[recv, mname, a_arr, t_arr]);
                            }
                            return self.helper("callMethod", &[recv, mname, a_arr]);
                        }
                        if type_args.is_empty()
                            && matches!(
                                method.as_str(),
                                "parse"
                                    | "collect"
                                    | "fetch_one"
                                    | "fetch_optional"
                                    | "fetch_all"
                                    | "json"
                            )
                        {
                            let recv = self.emit_expr(receiver)?;
                            let mname = self.string(method)?;
                            let mut a_regs = Vec::with_capacity(args.len());
                            for a in args {
                                a_regs.push(self.emit_expr(a)?);
                            }
                            let a_arr = self.js_array(&a_regs)?;
                            let t_reg = self.string(target)?;
                            let t_arr = self.js_array(&[t_reg])?;
                            return self.helper("callMethod", &[recv, mname, a_arr, t_arr]);
                        }
                    }
                    ExprKind::Call { callee, args } => {
                        if let ExprKind::Path(p) = &callee.kind {
                            let last = p.segments.last().map(|s| s.ident.as_str());
                            let parent = p
                                .segments
                                .len()
                                .checked_sub(2)
                                .and_then(|i| p.segments.get(i))
                                .map(|s| s.ident.as_str());
                            if args.is_empty()
                                && last == Some("default")
                                && parent == Some("Default")
                            {
                                let p1 = self.string("Default")?;
                                let p2 = self.string("default")?;
                                let p_arr = self.js_array(&[p1, p2])?;
                                let empty_args = self.js_array(&[])?;
                                let t_reg = self.string(target)?;
                                let t_arr = self.js_array(&[t_reg])?;
                                return self.helper("callAssoc", &[p_arr, empty_args, t_arr]);
                            }
                            let has_explicit_ta = p
                                .segments
                                .last()
                                .is_some_and(|s| !s.type_args.is_empty());
                            if !has_explicit_ta
                                && matches!(
                                    last,
                                    Some(
                                        "from_str"
                                            | "from_value"
                                            | "from_slice"
                                            | "decode"
                                            | "query_as"
                                            | "query_scalar"
                                    )
                                )
                            {
                                let raw: Vec<String> =
                                    p.segments.iter().map(|s| s.ident.clone()).collect();
                                let resolved = self.resolve_segments(&raw);
                                let mut p_regs = Vec::with_capacity(resolved.len());
                                for seg in &resolved {
                                    p_regs.push(self.string(seg)?);
                                }
                                let p_arr = self.js_array(&p_regs)?;
                                let mut a_regs = Vec::with_capacity(args.len());
                                for a in args {
                                    a_regs.push(self.emit_expr(a)?);
                                }
                                let a_arr = self.js_array(&a_regs)?;
                                let t_reg = self.string(target)?;
                                let t_arr = self.js_array(&[t_reg])?;
                                return self.helper("callAssoc", &[p_arr, a_arr, t_arr]);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.emit_expr(expr)
    }

    pub(super) fn emit_expr(&mut self, expr: &Expr) -> R<Reg> {
        match &expr.kind {
            ExprKind::Literal(lit) => self.emit_literal(lit),
            ExprKind::Path(path) => self.emit_path(path),
            ExprKind::QualifiedPath { ty, item, .. } => {
                let base = type_base_name(ty);
                self.emit_path(&Path {
                    leading_colon: false,
                    segments: vec![
                        PathSegment {
                            ident: base,
                            type_args: Vec::new(),
                        },
                        PathSegment {
                            ident: item.clone(),
                            type_args: Vec::new(),
                        },
                    ],
                    span: expr.span,
                })
            }
            ExprKind::Tuple(elems) => {
                if elems.is_empty() {
                    return self.undefined();
                }
                let mut regs = Vec::with_capacity(elems.len());
                for e in elems {
                    regs.push(self.emit_expr(e)?);
                }
                let arr = self.js_array(&regs)?;
                self.helper("mkTuple", &[arr])
            }
            ExprKind::Array(elems) => {
                let mut regs = Vec::with_capacity(elems.len());
                for e in elems {
                    regs.push(self.emit_expr(e)?);
                }
                let arr = self.js_array(&regs)?;
                self.helper("mkVec", &[arr])
            }
            ExprKind::Repeat { elem, count } => {
                let er = self.emit_expr(elem)?;
                let cr = self.emit_expr(count)?;
                self.helper("mkRepeat", &[er, cr])
            }
            ExprKind::Struct { path, fields, rest } => {
                let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
                let resolved = self.resolve_segments(&raw);
                let struct_info = {
                    let q = resolved.join("::");
                    let short = resolved.last().map(|s| s.as_str()).unwrap_or("");
                    self.syms
                        .structs
                        .get(&q)
                        .or_else(|| self.syms.structs.get(short))
                };
                let obj = self.new_js_obj()?;
                for f in fields {
                    let fhint = struct_info.and_then(|si| {
                        si.fields
                            .iter()
                            .find(|(fnm, _)| fnm == &f.name)
                            .map(|(_, ft)| ft.as_str())
                    });
                    let vr = self.emit_expr_with_hint(&f.value, fhint)?;
                    self.set_prop(obj, &f.name, vr)?;
                }
                // Check if this is an enum struct variant `Enum::Variant { ... }`
                if let Some((ename, vname, VariantKind::Struct)) =
                    self.syms.lookup_enum_variant(&self.mod_prefix, &resolved)
                {
                    let en = self.string(&ename)?;
                    let vn = self.string(&vname)?;
                    return self.helper("mkEnumStruct", &[en, vn, obj]);
                }
                let name = resolved.last().map(|s| s.as_str()).unwrap_or("");
                let sname = self.string(name)?;
                if let Some(ref re) = rest {
                    let rr = self.emit_expr(re)?;
                    self.helper("mkStruct", &[sname, obj, rr])
                } else {
                    self.helper("mkStruct", &[sname, obj])
                }
            }
            ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => self.emit_block(b),
            ExprKind::AsyncBlock { is_move, block } => self.emit_async_block(*is_move, block),
            ExprKind::Unary { op, expr: inner } => match op {
                UnOp::Neg => {
                    let k = self.infer_num_kind(inner);
                    let r = self.emit_expr(inner)?;
                    if k.is_numeric() {
                        let dst = self.alloc()?;
                        self.emit(Instr::Neg { dst, a: r })?;
                        Ok(dst)
                    } else {
                        self.helper("neg", &[r])
                    }
                }
                UnOp::Not => {
                    let r = self.emit_expr(inner)?;
                    self.helper("not", &[r])
                }
                UnOp::Deref => {
                    let r = self.emit_expr(inner)?;
                    self.helper("derefOne", &[r])
                }
            },
            ExprKind::Ref {
                is_mut,
                expr: inner,
            } => {
                if *is_mut {
                    self.emit_mut_ref(inner)
                } else {
                    self.emit_expr(inner)
                }
            }
            ExprKind::Binary { op, left, right } => self.emit_binary(*op, left, right),
            ExprKind::Assign { op, left, right } => self.emit_assign(*op, left, right),
            ExprKind::Cast { expr: inner, ty } => {
                let v = self.emit_expr(inner)?;
                let tname = self.string(&type_base_name(ty))?;
                self.helper("cast", &[v, tname])
            }
            ExprKind::Try(inner) => {
                let v = self.emit_expr(inner)?;
                let ok = self.helper("tryOk", &[v])?;
                let j_ok = self.jump_if_true(ok)?;
                let err_val = if let Some(ref et) = self.ret_err_type.clone() {
                    let et_r = self.string(et)?;
                    self.helper("tryErr", &[v, et_r])?
                } else {
                    self.helper("tryErr", &[v])?
                };
                self.emit(Instr::Return { src: err_val })?;
                self.patch(j_ok, self.here())?;
                self.helper("tryUnwrap", &[v])
            }
            ExprKind::Await(inner) => {
                let fut = self.emit_expr(inner)?;
                if self.proto.is_async {
                    let dst = self.alloc()?;
                    self.emit(Instr::Await { dst, val: fut })?;
                    Ok(dst)
                } else {
                    Ok(fut)
                }
            }
            ExprKind::Call { callee, args } => self.emit_call(callee, args),
            ExprKind::MethodCall {
                receiver,
                method,
                type_args,
                args,
            } => self.emit_method_call(receiver, method, type_args, args),
            ExprKind::Field { receiver, field } => {
                let recv = self.emit_expr(receiver)?;
                let fname = self.string(field)?;
                self.helper("getField", &[recv, fname])
            }
            ExprKind::TupleIndex { receiver, index } => {
                let recv = self.emit_expr(receiver)?;
                let idx = self.small_int(*index as i32)?;
                self.helper("getTupleIdx", &[recv, idx])
            }
            ExprKind::Index { receiver, index } => {
                let recv = self.emit_expr(receiver)?;
                let idx = self.emit_expr(index)?;
                self.helper("index", &[recv, idx])
            }
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let sr = match start {
                    Some(e) => self.emit_expr(e)?,
                    None => self.undefined()?,
                };
                let er = match end {
                    Some(e) => self.emit_expr(e)?,
                    None => self.undefined()?,
                };
                let inc = self.boolean(*inclusive)?;
                self.helper("mkRange", &[sr, er, inc])
            }
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } => self.emit_if(cond, then_block, else_branch.as_deref()),
            ExprKind::Match { scrutinee, arms } => self.emit_match(scrutinee, arms),
            ExprKind::Loop { label, body } => self.emit_loop(label.clone(), body),
            ExprKind::While { label, cond, body } => self.emit_while(label.clone(), cond, body),
            ExprKind::For {
                label,
                pat,
                iter,
                body,
            } => self.emit_for(label.clone(), pat, iter, body),
            ExprKind::Break { label, value } => self.emit_break(label.as_deref(), value.as_deref()),
            ExprKind::Continue { label } => self.emit_continue(label.as_deref()),
            ExprKind::Return(val) => {
                let r = match val {
                    Some(e) => self.emit_expr(e)?,
                    None => self.undefined()?,
                };
                self.emit(Instr::Return { src: r })?;
                self.undefined()
            }
            ExprKind::Closure {
                is_async,
                is_move,
                params,
                body,
                ..
            } => self.emit_closure(*is_async, *is_move, params, body),
            ExprKind::MacroCall(mc) => self.emit_macro_call(mc),
        }
    }

    pub(super) fn emit_path(&mut self, path: &Path) -> R<Reg> {
        let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
        // 1. Local variable in lexical scope
        if raw.len() == 1 && !path.leading_colon {
            if let Some(lv) = self.lookup_local(&raw[0]) {
                return self.load_local(lv);
            }
            if raw[0] == "None" {
                return self.prop(self.r_rt, "None");
            }
            if raw[0] == "Some" {
                return self.prop(self.r_rt, "Some");
            }
            if raw[0] == "Ok" {
                return self.prop(self.r_rt, "Ok");
            }
            if raw[0] == "Err" {
                return self.prop(self.r_rt, "Err");
            }
        }
        // 2. Free function global
        if let Some((_, slot)) = self.syms.lookup_fn_global(&self.mod_prefix, &raw) {
            let dst = self.alloc()?;
            self.emit(Instr::LoadGlobal { dst, idx: slot })?;
            return Ok(dst);
        }
        // 3. Static or const global
        if let Some((_, slot)) = self.syms.lookup_static_global(&self.mod_prefix, &raw) {
            let dst = self.alloc()?;
            self.emit(Instr::LoadGlobal { dst, idx: slot })?;
            return Ok(dst);
        }
        // 4. Unit struct literal as value (`struct Marker; let m = Marker;`)
        let resolved = self.resolve_segments(&raw);
        let last = resolved.last().map(|s| s.as_str()).unwrap_or("");
        if resolved.len() == 1 {
            if let Some(info) = self.syms.structs.get(last) {
                if info.is_unit {
                    let sn = self.string(&info.name)?;
                    let obj = self.new_js_obj()?;
                    return self.helper("mkStruct", &[sn, obj]);
                }
            }
        }
        // 5. Enum unit variant, associated constant, stdlib constant (`Ordering::Less`, `i32::MAX`, `std::f64::consts::PI`, `SystemTime::UNIX_EPOCH`)
        let mut seg_regs = Vec::with_capacity(resolved.len());
        for seg in &resolved {
            seg_regs.push(self.string(seg)?);
        }
        let arr = self.js_array(&seg_regs)?;
        self.helper("resolveConstPath", &[arr])
    }

    fn emit_mut_ref(&mut self, inner: &Expr) -> R<Reg> {
        match &inner.kind {
            ExprKind::Path(p) if p.segments.len() == 1 && !p.leading_colon => {
                if let Some(lv) = self.lookup_local(&p.segments[0].ident) {
                    if lv.boxed {
                        return Ok(lv.reg);
                    }
                    let v = self.load_local(lv)?;
                    return self.helper("mkRef", &[v]);
                }
            }
            ExprKind::Field { receiver, field } => {
                let obj = self.emit_expr(receiver)?;
                let fname = self.string(field)?;
                return self.helper("mkPropRef", &[obj, fname]);
            }
            ExprKind::TupleIndex { receiver, index } => {
                let obj = self.emit_expr(receiver)?;
                let idx = self.small_int(*index as i32)?;
                return self.helper("mkTupleIdxRef", &[obj, idx]);
            }
            ExprKind::Index { receiver, index } => {
                let obj = self.emit_expr(receiver)?;
                let idx = self.emit_expr(index)?;
                return self.helper("mkIdxRef", &[obj, idx]);
            }
            ExprKind::Unary {
                op: UnOp::Deref,
                expr: ptr,
            } => {
                return self.emit_expr(ptr);
            }
            _ => {}
        }
        let v = self.emit_expr(inner)?;
        self.helper("mkRef", &[v])
    }

    fn emit_binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> R<Reg> {
        // Short-circuit `&&` and `||`
        if op == BinOp::And {
            let l = self.emit_expr(left)?;
            let dst = self.alloc()?;
            self.emit(Instr::LoadBool { dst, val: false })?;
            let j_false = self.jump_if_false(l)?;
            let r = self.emit_expr(right)?;
            self.emit(Instr::Move { dst, src: r })?;
            self.patch(j_false, self.here())?;
            return Ok(dst);
        }
        if op == BinOp::Or {
            let l = self.emit_expr(left)?;
            let dst = self.alloc()?;
            self.emit(Instr::LoadBool { dst, val: true })?;
            let j_true = self.jump_if_true(l)?;
            let r = self.emit_expr(right)?;
            self.emit(Instr::Move { dst, src: r })?;
            self.patch(j_true, self.here())?;
            return Ok(dst);
        }

        let lk = self.infer_num_kind(left);
        let rk = self.infer_num_kind(right);
        let both_num = lk.is_numeric() && rk.is_numeric();
        let is_float = lk == NumKind::Float || rk == NumKind::Float;
        let is_int = !is_float && (lk == NumKind::Int || rk == NumKind::Int);

        let a = self.emit_expr(left)?;
        let b = self.emit_expr(right)?;

        if both_num {
            let dst = self.alloc()?;
            match op {
                BinOp::Add => {
                    self.emit(Instr::Add { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Sub => {
                    self.emit(Instr::Sub { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Mul => {
                    self.emit(Instr::Mul { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Div if is_float => {
                    self.emit(Instr::Div { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Lt => {
                    self.emit(Instr::Lt { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Le => {
                    self.emit(Instr::Le { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Gt => {
                    self.emit(Instr::Gt { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Ge => {
                    self.emit(Instr::Ge { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Eq => {
                    self.emit(Instr::Eq { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::Ne => {
                    self.emit(Instr::Ne { dst, a, b })?;
                    return Ok(dst);
                }
                BinOp::BitAnd if is_int => {
                    self.emit(Instr::Bitwise {
                        dst,
                        a,
                        b,
                        op: BitwiseOp::And,
                    })?;
                    return Ok(dst);
                }
                BinOp::BitOr if is_int => {
                    self.emit(Instr::Bitwise {
                        dst,
                        a,
                        b,
                        op: BitwiseOp::Or,
                    })?;
                    return Ok(dst);
                }
                BinOp::BitXor if is_int => {
                    self.emit(Instr::Bitwise {
                        dst,
                        a,
                        b,
                        op: BitwiseOp::Xor,
                    })?;
                    return Ok(dst);
                }
                BinOp::Shl if is_int => {
                    self.emit(Instr::Bitwise {
                        dst,
                        a,
                        b,
                        op: BitwiseOp::Shl,
                    })?;
                    return Ok(dst);
                }
                BinOp::Shr if is_int => {
                    self.emit(Instr::Bitwise {
                        dst,
                        a,
                        b,
                        op: BitwiseOp::Shr,
                    })?;
                    return Ok(dst);
                }
                _ => {}
            }
        }

        match op {
            BinOp::Add => self.helper("add", &[a, b]),
            BinOp::Sub => self.helper("sub", &[a, b]),
            BinOp::Mul => self.helper("mul", &[a, b]),
            BinOp::Div => {
                let int_flag = self.boolean(!is_float)?;
                self.helper("div", &[a, b, int_flag])
            }
            BinOp::Rem => {
                let int_flag = self.boolean(!is_float)?;
                self.helper("rem", &[a, b, int_flag])
            }
            BinOp::BitAnd => self.helper("bitAnd", &[a, b]),
            BinOp::BitOr => self.helper("bitOr", &[a, b]),
            BinOp::BitXor => self.helper("bitXor", &[a, b]),
            BinOp::Shl => self.helper("shl", &[a, b]),
            BinOp::Shr => self.helper("shr", &[a, b]),
            BinOp::Eq => self.helper("eq", &[a, b]),
            BinOp::Ne => self.helper("ne", &[a, b]),
            BinOp::Lt => self.helper("lt", &[a, b]),
            BinOp::Le => self.helper("le", &[a, b]),
            BinOp::Gt => self.helper("gt", &[a, b]),
            BinOp::Ge => self.helper("ge", &[a, b]),
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn emit_assign(&mut self, op: Option<BinOp>, left: &Expr, right: &Expr) -> R<Reg> {
        let apply_op = |this: &mut Self, cur: Reg, rhs: Reg, lk: NumKind, rk: NumKind| -> R<Reg> {
            let Some(bin_op) = op else {
                return Ok(rhs);
            };
            let both_num = lk.is_numeric() && rk.is_numeric();
            let is_float = lk == NumKind::Float || rk == NumKind::Float;
            if both_num {
                let dst = this.alloc()?;
                match bin_op {
                    BinOp::Add => {
                        this.emit(Instr::Add { dst, a: cur, b: rhs })?;
                        return Ok(dst);
                    }
                    BinOp::Sub => {
                        this.emit(Instr::Sub { dst, a: cur, b: rhs })?;
                        return Ok(dst);
                    }
                    BinOp::Mul => {
                        this.emit(Instr::Mul { dst, a: cur, b: rhs })?;
                        return Ok(dst);
                    }
                    BinOp::Div if is_float => {
                        this.emit(Instr::Div { dst, a: cur, b: rhs })?;
                        return Ok(dst);
                    }
                    _ => {}
                }
            }
            match bin_op {
                BinOp::Add => this.helper("add", &[cur, rhs]),
                BinOp::Sub => this.helper("sub", &[cur, rhs]),
                BinOp::Mul => this.helper("mul", &[cur, rhs]),
                BinOp::Div => {
                    let f = this.boolean(!is_float)?;
                    this.helper("div", &[cur, rhs, f])
                }
                BinOp::Rem => {
                    let f = this.boolean(!is_float)?;
                    this.helper("rem", &[cur, rhs, f])
                }
                BinOp::BitAnd => this.helper("bitAnd", &[cur, rhs]),
                BinOp::BitOr => this.helper("bitOr", &[cur, rhs]),
                BinOp::BitXor => this.helper("bitXor", &[cur, rhs]),
                BinOp::Shl => this.helper("shl", &[cur, rhs]),
                BinOp::Shr => this.helper("shr", &[cur, rhs]),
                _ => Ok(rhs),
            }
        };

        let rk = self.infer_num_kind(right);
        match &left.kind {
            ExprKind::Path(p) if p.segments.len() == 1 && !p.leading_colon => {
                let name = &p.segments[0].ident;
                if let Some(lv) = self.lookup_local(name) {
                    let rhs = self.emit_expr(right)?;
                    let val = if op.is_some() {
                        let cur = self.load_local(lv)?;
                        apply_op(self, cur, rhs, lv.num_kind, rk)?
                    } else {
                        rhs
                    };
                    self.store_local(lv, val)?;
                    return self.undefined();
                }
                let raw = vec![name.clone()];
                if let Some((_, slot)) = self.syms.lookup_static_global(&self.mod_prefix, &raw) {
                    let rhs = self.emit_expr(right)?;
                    let val = if op.is_some() {
                        let cur = self.alloc()?;
                        self.emit(Instr::LoadGlobal { dst: cur, idx: slot })?;
                        apply_op(self, cur, rhs, NumKind::Unknown, rk)?
                    } else {
                        rhs
                    };
                    self.emit(Instr::StoreGlobal { idx: slot, src: val })?;
                    return self.undefined();
                }
                return Err(format!("Rust: assignment to undeclared variable `{name}`"));
            }
            ExprKind::Unary {
                op: UnOp::Deref,
                expr: ptr,
            } => {
                let pref = self.emit_expr(ptr)?;
                let rhs = self.emit_expr(right)?;
                let val = if op.is_some() {
                    let cur = self.helper("rget", &[pref])?;
                    apply_op(self, cur, rhs, NumKind::Unknown, rk)?
                } else {
                    rhs
                };
                self.helper("rset", &[pref, val])?;
                self.undefined()
            }
            ExprKind::Field { receiver, field } => {
                let obj = self.emit_expr(receiver)?;
                let fname = self.string(field)?;
                let rhs = self.emit_expr(right)?;
                let val = if op.is_some() {
                    let cur = self.helper("getField", &[obj, fname])?;
                    apply_op(self, cur, rhs, NumKind::Unknown, rk)?
                } else {
                    rhs
                };
                self.helper("setField", &[obj, fname, val])?;
                self.undefined()
            }
            ExprKind::TupleIndex { receiver, index } => {
                let obj = self.emit_expr(receiver)?;
                let idx = self.small_int(*index as i32)?;
                let rhs = self.emit_expr(right)?;
                let val = if op.is_some() {
                    let cur = self.helper("getTupleIdx", &[obj, idx])?;
                    apply_op(self, cur, rhs, NumKind::Unknown, rk)?
                } else {
                    rhs
                };
                self.helper("setTupleIdx", &[obj, idx, val])?;
                self.undefined()
            }
            ExprKind::Index { receiver, index } => {
                let obj = self.emit_expr(receiver)?;
                let idx = self.emit_expr(index)?;
                let rhs = self.emit_expr(right)?;
                let val = if op.is_some() {
                    let cur = self.helper("index", &[obj, idx])?;
                    apply_op(self, cur, rhs, NumKind::Unknown, rk)?
                } else {
                    rhs
                };
                self.helper("setIndex", &[obj, idx, val])?;
                self.undefined()
            }
            _ => Err(format!("Rust: invalid assignment target: {:?}", left.kind)),
        }
    }

    fn emit_call(&mut self, callee: &Expr, args: &[Expr]) -> R<Reg> {
        // Check special path calls: prelude constructors, tuple structs, enum tuple variants,
        // direct free functions, and associated functions (`Type::method`, `std::fs::read_to_string`).
        if let ExprKind::Path(path) = &callee.kind {
            let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
            let type_args: Vec<String> = path
                .segments
                .iter()
                .rev()
                .find(|s| !s.type_args.is_empty())
                .map(|s| {
                    s.type_args
                        .iter()
                        .map(|t| self.syms.type_desc_resolved(&self.mod_prefix, t))
                        .collect()
                })
                .unwrap_or_default();

            // 1. Local variable holding a closure/fn pointer (auto-derefs Box<dyn Fn> / Arc<dyn Fn>!)
            if raw.len() == 1 && !path.leading_colon {
                if let Some(lv) = self.lookup_local(&raw[0]) {
                    let f = self.load_local(lv)?;
                    let mut aregs = Vec::with_capacity(args.len());
                    for a in args {
                        aregs.push(self.emit_expr(a)?);
                    }
                    let a_arr = self.js_array(&aregs)?;
                    return self.helper("callFn", &[f, a_arr]);
                }
            }

            // 2. Direct free function global (`foo(a, b)`, `mod1::bar(a)`) -> ZERO wrapper `Instr::Call`!
            if let Some((_, slot)) = self.syms.lookup_fn_global(&self.mod_prefix, &raw) {
                let f = self.alloc()?;
                self.emit(Instr::LoadGlobal { dst: f, idx: slot })?;
                let mut aregs = Vec::with_capacity(args.len());
                for a in args {
                    aregs.push(self.emit_expr(a)?);
                }
                return self.call_raw(f, &aregs);
            }

            // 3. Prelude constructors `Some(x)`, `Ok(x)`, `Err(x)`
            let resolved = self.resolve_segments(&raw);
            let last = resolved.last().map(|s| s.as_str()).unwrap_or("");
            if resolved.len() == 1 {
                match last {
                    "Some" | "Ok" | "Err" => {
                        let arg = if let Some(a) = args.first() {
                            self.emit_expr(a)?
                        } else {
                            self.undefined()?
                        };
                        return self.helper(last, &[arg]);
                    }
                    _ => {}
                }
                // Tuple struct constructor `Pair(1, 2)`
                if let Some(info) = self.syms.structs.get(last) {
                    if info.is_tuple {
                        let sname = self.string(&info.name)?;
                        let mut aregs = Vec::with_capacity(args.len());
                        for a in args {
                            aregs.push(self.emit_expr(a)?);
                        }
                        let arr = self.js_array(&aregs)?;
                        return self.helper("mkTupleStruct", &[sname, arr]);
                    }
                }
            }

            // 4. Enum tuple variant constructor `MyEnum::Variant(a, b)`
            if let Some((ename, vname, VariantKind::Tuple)) =
                self.syms.lookup_enum_variant(&self.mod_prefix, &resolved)
            {
                let en = self.string(&ename)?;
                let vn = self.string(&vname)?;
                let mut aregs = Vec::with_capacity(args.len());
                for a in args {
                    aregs.push(self.emit_expr(a)?);
                }
                let arr = self.js_array(&aregs)?;
                return self.helper("mkEnumTuple", &[en, vn, arr]);
            }

            // 5. Associated function / stdlib module call (`String::from`, `Vec::new`, `Point::new`, `fs::read_to_string`, `Router::new`, ...)
            let mut p_regs = Vec::with_capacity(resolved.len());
            for seg in &resolved {
                p_regs.push(self.string(seg)?);
            }
            let p_arr = self.js_array(&p_regs)?;
            let mut a_regs = Vec::with_capacity(args.len());
            for a in args {
                a_regs.push(self.emit_expr(a)?);
            }
            let a_arr = self.js_array(&a_regs)?;
            if type_args.is_empty() {
                return self.helper("callAssoc", &[p_arr, a_arr]);
            } else {
                let mut t_regs = Vec::with_capacity(type_args.len());
                for t in &type_args {
                    t_regs.push(self.string(t)?);
                }
                let t_arr = self.js_array(&t_regs)?;
                return self.helper("callAssoc", &[p_arr, a_arr, t_arr]);
            }
        }

        // Qualified path call `<T as Trait>::method(args)`
        if let ExprKind::QualifiedPath { ty, item, .. } = &callee.kind {
            let p1 = self.string(&type_base_name(ty))?;
            let p2 = self.string(item)?;
            let p_arr = self.js_array(&[p1, p2])?;
            let mut a_regs = Vec::with_capacity(args.len());
            for a in args {
                a_regs.push(self.emit_expr(a)?);
            }
            let a_arr = self.js_array(&a_regs)?;
            return self.helper("callAssoc", &[p_arr, a_arr]);
        }

        // General callable expression `(get_fn())(args)` or `(obj.cb)(args)`
        let f = self.emit_expr(callee)?;
        let mut aregs = Vec::with_capacity(args.len());
        for a in args {
            aregs.push(self.emit_expr(a)?);
        }
        let a_arr = self.js_array(&aregs)?;
        self.helper("callFn", &[f, a_arr])
    }

    fn emit_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        type_args: &[Type],
        args: &[Expr],
    ) -> R<Reg> {
        // When the receiver is a boxed local OR a field/index place targeted by a `&mut self`
        // method (e.g. `opt.take()`, `issue.status.transition_to(...)`), pass a ref cell so
        // `R.callMethod` can mutate the place in-situ (`*self = ...`) or auto-deref it!
        let needs_mut_place = matches!(
            method,
            "take" | "replace" | "get_or_insert" | "get_or_insert_with"
        ) || self.syms.mut_self_methods.contains(method);
        let recv = match &receiver.kind {
            ExprKind::Path(p) if p.segments.len() == 1 && !p.leading_colon => {
                if let Some(lv) = self.lookup_local(&p.segments[0].ident) {
                    if lv.boxed {
                        lv.reg
                    } else {
                        self.emit_expr(receiver)?
                    }
                } else {
                    self.emit_expr(receiver)?
                }
            }
            ExprKind::Field { .. } | ExprKind::TupleIndex { .. } | ExprKind::Index { .. }
                if needs_mut_place =>
            {
                self.emit_mut_ref(receiver)?
            }
            _ => self.emit_expr(receiver)?,
        };
        let mname = self.string(method)?;
        let mut a_regs = Vec::with_capacity(args.len());
        for a in args {
            a_regs.push(self.emit_expr(a)?);
        }
        let a_arr = self.js_array(&a_regs)?;
        if type_args.is_empty() {
            self.helper("callMethod", &[recv, mname, a_arr])
        } else {
            let mut t_regs = Vec::with_capacity(type_args.len());
            for t in type_args {
                t_regs.push(self.string(&self.syms.type_desc_resolved(&self.mod_prefix, t))?);
            }
            let t_arr = self.js_array(&t_regs)?;
            self.helper("callMethod", &[recv, mname, a_arr, t_arr])
        }
    }

    // ---- Control flow ----

    fn emit_if_cond(&mut self, cond: &IfCond, fails: &mut Vec<usize>) -> R<()> {
        match cond {
            IfCond::Expr(e) => {
                let c = self.emit_expr(e)?;
                fails.push(self.jump_if_false(c)?);
                Ok(())
            }
            IfCond::Let { pat, expr } => {
                let v = self.emit_expr(expr)?;
                self.emit_pat_match(pat, v, fails)
            }
            IfCond::Chain(cs) => {
                for c in cs {
                    self.emit_if_cond(c, fails)?;
                }
                Ok(())
            }
        }
    }

    fn emit_if(
        &mut self,
        cond: &IfCond,
        then_block: &Block,
        else_branch: Option<&Expr>,
    ) -> R<Reg> {
        let dst = self.undefined()?;
        self.push_scope();
        let mut fails = Vec::new();
        self.emit_if_cond(cond, &mut fails)?;
        let then_val = self.emit_block(then_block)?;
        self.emit(Instr::Move {
            dst,
            src: then_val,
        })?;
        self.pop_scope();
        let j_end = self.jump()?;
        let else_pc = self.here();
        for fj in fails {
            self.patch(fj, else_pc)?;
        }
        if let Some(eb) = else_branch {
            let ev = self.emit_expr(eb)?;
            self.emit(Instr::Move { dst, src: ev })?;
        }
        self.patch(j_end, self.here())?;
        Ok(dst)
    }

    fn emit_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> R<Reg> {
        let val = self.emit_expr(scrutinee)?;
        let dst = self.undefined()?;
        let mut end_jumps = Vec::with_capacity(arms.len());
        for arm in arms {
            self.push_scope();
            let mut fails = Vec::new();
            self.emit_pat_match(&arm.pat, val, &mut fails)?;
            if let Some(ref guard) = arm.guard {
                let g = self.emit_expr(guard)?;
                fails.push(self.jump_if_false(g)?);
            }
            let body_val = self.emit_expr(&arm.body)?;
            self.emit(Instr::Move {
                dst,
                src: body_val,
            })?;
            self.pop_scope();
            end_jumps.push(self.jump()?);
            let next_pc = self.here();
            for fj in fails {
                self.patch(fj, next_pc)?;
            }
        }
        let end_pc = self.here();
        for ej in end_jumps {
            self.patch(ej, end_pc)?;
        }
        Ok(dst)
    }

    fn emit_loop(&mut self, label: Option<String>, body: &Block) -> R<Reg> {
        let result_reg = self.undefined()?;
        let head_pc = self.here();
        self.loops.push(LoopCtx {
            label,
            result_reg: Some(result_reg),
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        let _ = self.emit_block(body)?;
        self.emit(Instr::Jump { target: head_pc })?;
        let end_pc = self.here();
        let ctx = self.loops.pop().unwrap();
        for b in ctx.breaks {
            self.patch(b, end_pc)?;
        }
        for c in ctx.continues {
            self.patch(c, head_pc)?;
        }
        Ok(result_reg)
    }

    fn emit_while(&mut self, label: Option<String>, cond: &IfCond, body: &Block) -> R<Reg> {
        let head_pc = self.here();
        self.loops.push(LoopCtx {
            label,
            result_reg: None,
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        self.push_scope();
        let mut fails = Vec::new();
        self.emit_if_cond(cond, &mut fails)?;
        let _ = self.emit_block(body)?;
        self.pop_scope();
        self.emit(Instr::Jump { target: head_pc })?;
        let end_pc = self.here();
        for fj in fails {
            self.patch(fj, end_pc)?;
        }
        let ctx = self.loops.pop().unwrap();
        for b in ctx.breaks {
            self.patch(b, end_pc)?;
        }
        for c in ctx.continues {
            self.patch(c, head_pc)?;
        }
        self.undefined()
    }

    fn emit_for(
        &mut self,
        label: Option<String>,
        pat: &Pat,
        iter_expr: &Expr,
        body: &Block,
    ) -> R<Reg> {
        // Fast path: `for i in start..end` / `for i in start..=end` with integer bounds & simple ident!
        // Compiles to a pure register counter loop with ZERO allocations and full x86-64 Tier-C JIT!
        if let (
            PatKind::Ident {
                name,
                subpat: None,
                ..
            },
            ExprKind::Range {
                start: Some(start_e),
                end: Some(end_e),
                inclusive,
            },
        ) = (&pat.kind, &iter_expr.kind)
        {
            if !self.boxed_vars.contains(name)
                && self.infer_num_kind(start_e) == NumKind::Int
                && self.infer_num_kind(end_e) == NumKind::Int
            {
                let start_r = self.emit_expr(start_e)?;
                let end_r = self.emit_expr(end_e)?;
                let i_reg = self.alloc()?;
                self.emit(Instr::Move {
                    dst: i_reg,
                    src: start_r,
                })?;

                self.push_scope();
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    LocalVar {
                        reg: i_reg,
                        boxed: false,
                        num_kind: NumKind::Int,
                    },
                );

                let head_pc = self.here();
                let j_exit = if *inclusive {
                    self.emit(Instr::JumpIfNotLe {
                        a: i_reg,
                        b: end_r,
                        target: 0,
                    })?
                } else {
                    self.emit(Instr::JumpIfNotLt {
                        a: i_reg,
                        b: end_r,
                        target: 0,
                    })?
                };

                self.loops.push(LoopCtx {
                    label,
                    result_reg: None,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                let _ = self.emit_block(body)?;
                let step_pc = self.here();
                self.emit(Instr::AddInt {
                    dst: i_reg,
                    a: i_reg,
                    imm: 1,
                    upd: false,
                })?;
                self.emit(Instr::Jump { target: head_pc })?;
                let end_pc = self.here();
                self.patch(j_exit, end_pc)?;
                let ctx = self.loops.pop().unwrap();
                for b in ctx.breaks {
                    self.patch(b, end_pc)?;
                }
                for c in ctx.continues {
                    self.patch(c, step_pc)?;
                }
                self.pop_scope();
                return self.undefined();
            }
        }

        // General iterator `for pat in iter_expr { body }`
        let raw_iter = self.emit_expr(iter_expr)?;
        let iter_reg = self.helper("intoIter", &[raw_iter])?;
        let done_sentinel = self.prop(self.r_rt, "ITER_DONE")?;
        let head_pc = self.here();
        let step_val = self.helper("iterNext", &[iter_reg])?;
        let is_done = self.alloc()?;
        self.emit(Instr::Eq {
            dst: is_done,
            a: step_val,
            b: done_sentinel,
        })?;
        let j_exit = self.jump_if_true(is_done)?;

        self.loops.push(LoopCtx {
            label,
            result_reg: None,
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        self.push_scope();
        self.emit_pat_bind(pat, step_val, NumKind::Unknown)?;
        let _ = self.emit_block(body)?;
        self.pop_scope();
        self.emit(Instr::Jump { target: head_pc })?;
        let end_pc = self.here();
        self.patch(j_exit, end_pc)?;
        let ctx = self.loops.pop().unwrap();
        for b in ctx.breaks {
            self.patch(b, end_pc)?;
        }
        for c in ctx.continues {
            self.patch(c, head_pc)?;
        }
        self.undefined()
    }

    fn emit_break(&mut self, label: Option<&str>, value: Option<&Expr>) -> R<Reg> {
        let val_reg = match value {
            Some(e) => Some(self.emit_expr(e)?),
            None => None,
        };
        let idx = self
            .find_loop(label)
            .ok_or_else(|| "Rust: `break` outside of loop".to_owned())?;
        if let (Some(vr), Some(res_reg)) = (val_reg, self.loops[idx].result_reg) {
            self.emit(Instr::Move {
                dst: res_reg,
                src: vr,
            })?;
        }
        let j = self.jump()?;
        self.loops[idx].breaks.push(j);
        self.undefined()
    }

    fn emit_continue(&mut self, label: Option<&str>) -> R<Reg> {
        let idx = self
            .find_loop(label)
            .ok_or_else(|| "Rust: `continue` outside of loop".to_owned())?;
        let j = self.jump()?;
        self.loops[idx].continues.push(j);
        self.undefined()
    }

    fn find_loop(&self, label: Option<&str>) -> Option<usize> {
        match label {
            None => self.loops.len().checked_sub(1),
            Some(lbl) => self
                .loops
                .iter()
                .rposition(|l| l.label.as_deref() == Some(lbl)),
        }
    }

    // ---- Closures & Async Blocks ----

    fn emit_closure(
        &mut self,
        is_async: bool,
        is_move: bool,
        params: &[(Pat, Option<Type>)],
        body: &Expr,
    ) -> R<Reg> {
        let pats: Vec<Pat> = params.iter().map(|(p, _)| p.clone()).collect();
        let analysis = analyze_closure(&pats, body, &self.syms.mut_self_methods);
        let captures: Vec<(String, LocalVar)> = analysis
            .free_vars
            .iter()
            .filter_map(|name| self.lookup_local(name).map(|lv| (name.clone(), lv)))
            .collect();

        let param_count =
            u16::try_from(params.len()).map_err(|_| "Rust: too many closure parameters")?;
        let mut sub = RsEmitter::new(
            "__rs_closure",
            param_count,
            is_async,
            self.rt_slot,
            self.program,
            self.syms,
            self.file,
            self.mod_prefix.clone(),
            self.self_type.clone(),
            analysis.boxed_vars,
        )?;
        sub.local_uses = self.local_uses.clone();

        // Bind captured variables from `reg 0` (`this` = env array of ref cells)
        for (i, (cname, outer_lv)) in captures.iter().enumerate() {
            let idx_r = sub.small_int(i as i32)?;
            let cell = sub.alloc()?;
            sub.emit(Instr::GetIndex {
                dst: cell,
                obj: 0,
                key: idx_r,
            })?;
            sub.bind_existing_cell(cname, cell, outer_lv.num_kind);
        }

        // Bind closure parameters
        for (i, (pat, ty)) in params.iter().enumerate() {
            let preg = (1 + i) as Reg;
            let nk = ty
                .as_ref()
                .map(NumKind::from_type)
                .unwrap_or(NumKind::Unknown);
            sub.emit_pat_bind(pat, preg, nk)?;
        }

        let ret = sub.emit_expr(body)?;
        sub.emit(Instr::Return { src: ret })?;
        let func_id = self.push_func(sub.finish())?;

        let fn_reg = self.alloc()?;
        self.emit(Instr::MakeFunc {
            dst: fn_reg,
            func_id,
        })?;

        if captures.is_empty() {
            Ok(fn_reg)
        } else {
            let mut cap_regs = Vec::with_capacity(captures.len());
            for (_, lv) in &captures {
                if is_move {
                    // Snapshot value into a fresh cell on `move` so loop-created move closures stay independent!
                    let v = self.load_local(*lv)?;
                    cap_regs.push(self.helper("mkRef", &[v])?);
                } else if lv.boxed {
                    cap_regs.push(lv.reg);
                } else {
                    let v = self.load_local(*lv)?;
                    cap_regs.push(self.helper("mkRef", &[v])?);
                }
            }
            let env_arr = self.js_array(&cap_regs)?;
            self.helper("mkClosure", &[fn_reg, env_arr])
        }
    }

    fn emit_async_block(&mut self, is_move: bool, block: &Block) -> R<Reg> {
        let analysis = analyze_fn(&[], block, &self.syms.mut_self_methods);
        let captures: Vec<(String, LocalVar)> = analysis
            .free_vars
            .iter()
            .filter_map(|name| self.lookup_local(name).map(|lv| (name.clone(), lv)))
            .collect();

        let mut sub = RsEmitter::new(
            "__rs_async_block",
            0,
            true,
            self.rt_slot,
            self.program,
            self.syms,
            self.file,
            self.mod_prefix.clone(),
            self.self_type.clone(),
            analysis.boxed_vars,
        )?;
        sub.local_uses = self.local_uses.clone();

        for (i, (cname, outer_lv)) in captures.iter().enumerate() {
            let idx_r = sub.small_int(i as i32)?;
            let cell = sub.alloc()?;
            sub.emit(Instr::GetIndex {
                dst: cell,
                obj: 0,
                key: idx_r,
            })?;
            sub.bind_existing_cell(cname, cell, outer_lv.num_kind);
        }

        let ret = sub.emit_block(block)?;
        sub.emit(Instr::Return { src: ret })?;
        let func_id = self.push_func(sub.finish())?;

        let fn_reg = self.alloc()?;
        self.emit(Instr::MakeFunc {
            dst: fn_reg,
            func_id,
        })?;

        let callable = if captures.is_empty() {
            fn_reg
        } else {
            let mut cap_regs = Vec::with_capacity(captures.len());
            for (_, lv) in &captures {
                if is_move || !lv.boxed {
                    let v = self.load_local(*lv)?;
                    cap_regs.push(self.helper("mkRef", &[v])?);
                } else {
                    cap_regs.push(lv.reg);
                }
            }
            let env_arr = self.js_array(&cap_regs)?;
            self.helper("mkClosure", &[fn_reg, env_arr])?
        };
        // Calling an async function immediately returns its Promise (Future)!
        self.call_raw(callable, &[])
    }
}
