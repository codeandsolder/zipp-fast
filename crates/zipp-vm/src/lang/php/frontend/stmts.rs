//! PHP statement & declaration lowering to ZIPP register bytecode.

use super::emitter::{LoopCtx, PhpEmitter, R, MAX_DEPTH, REG_CALLED, REG_FN, REG_SCOPE, REG_THIS};
use super::exprs::is_fresh_owned;
use super::symtable::{analyse_body, is_superglobal};
use crate::bytecode::{Instr, Reg};
use zipp_phpparse::ast::*;

impl<'a> PhpEmitter<'a> {
    /// Emit top-level statements: hoist unconditional functions first (with namespace/use tracking),
    /// then execute statements in source order.
    pub fn emit_top_level(&mut self, stmts: &[Stmt]) -> R<()> {
        // Pass 0: hoist top-level `declare(strict_types=1)` and top-level function decls
        let saved_ns = self.ns.clone();
        self.hoist_functions(stmts)?;
        self.ns = saved_ns;
        // Pass 1: emit statements in order, skipping already-hoisted top-level functions
        self.emit_stmts_inner(stmts, 0, true)?;
        Ok(())
    }

    fn hoist_functions(&mut self, stmts: &[Stmt]) -> R<()> {
        for s in stmts {
            match &s.kind {
                StmtKind::Declare { directives, body } => {
                    for (k, v) in directives {
                        if k.eq_ignore_ascii_case("strict_types") {
                            if let ExprKind::Int(1) = v.kind {
                                self.file.strict_types = true;
                            }
                        }
                    }
                    if let Some(b) = body {
                        self.hoist_functions(b)?;
                    }
                }
                StmtKind::Namespace { name, body } => {
                    if let Some(b) = body {
                        let prev = self.ns.clone();
                        self.ns.enter_namespace(name.as_deref());
                        self.hoist_functions(b)?;
                        self.ns = prev;
                    } else {
                        self.ns.enter_namespace(name.as_deref());
                    }
                }
                StmtKind::Use(items) => {
                    for it in items {
                        self.ns.add_use(it);
                    }
                }
                StmtKind::Function(decl) => {
                    let mark = self.mark();
                    self.emit_function_decl(decl)?;
                    self.release(mark);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn emit_stmts(&mut self, stmts: &[Stmt], depth: usize) -> R<()> {
        self.emit_stmts_inner(stmts, depth, false)
    }

    fn emit_stmts_inner(&mut self, stmts: &[Stmt], depth: usize, skip_hoisted_fns: bool) -> R<()> {
        if depth > MAX_DEPTH {
            return Err("PHP: statement nesting limit exceeded".into());
        }
        for s in stmts {
            if skip_hoisted_fns && matches!(s.kind, StmtKind::Function(_)) {
                continue;
            }
            let mark = self.mark();
            self.emit_stmt(s, depth + 1, skip_hoisted_fns)?;
            self.release(mark);
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt, depth: usize, top_level: bool) -> R<()> {
        match &stmt.kind {
            StmtKind::Nop => Ok(()),
            StmtKind::Label(name) => {
                let pc = self.here();
                let depth = self.handler_depth;
                self.labels.insert(name.clone(), (pc, depth));
                Ok(())
            }
            StmtKind::InlineHtml(html) => {
                let s = self.string(html)?;
                self.helper("echo", &[s])?;
                Ok(())
            }
            StmtKind::Expr(e) => {
                self.emit_expr(e, depth)?;
                Ok(())
            }
            StmtKind::Block(stmts) => self.emit_stmts(stmts, depth),
            StmtKind::Echo(exprs) => {
                for e in exprs {
                    let mark = self.mark();
                    let v = self.emit_expr(e, depth)?;
                    self.helper("echo", &[v])?;
                    self.release(mark);
                }
                Ok(())
            }
            StmtKind::Return(opt) => {
                let val = match opt {
                    Some(e) => {
                        let v = self.emit_expr(e, depth)?;
                        if !is_fresh_owned(e) {
                            self.helper("share", &[v])?
                        } else {
                            v
                        }
                    }
                    None => self.null()?,
                };
                self.emit(Instr::Return { src: val })?;
                Ok(())
            }
            StmtKind::Break(level_expr) => {
                let level = self.const_break_level(level_expr.as_ref())?;
                self.emit_break_or_continue(level, false)
            }
            StmtKind::Continue(level_expr) => {
                let level = self.const_break_level(level_expr.as_ref())?;
                self.emit_break_or_continue(level, true)
            }
            StmtKind::Global(vars) => {
                for v in vars {
                    let n = self.string(v)?;
                    let gref = self.helper("gref", &[n])?;
                    self.bind_var_ref(v, gref)?;
                }
                Ok(())
            }
            StmtKind::StaticVar(items) => {
                for (name, init) in items {
                    let n = self.string(name)?;
                    let init_fn = if let Some(expr) = init {
                        self.emit_default_thunk(expr)?
                    } else {
                        self.null()?
                    };
                    let cell = self.helper("stref", &[REG_FN, n, init_fn])?;
                    self.bind_var_ref(name, cell)?;
                }
                Ok(())
            }
            StmtKind::Unset(targets) => {
                for t in targets {
                    let mark = self.mark();
                    self.emit_unset_target(t, depth)?;
                    self.release(mark);
                }
                Ok(())
            }
            StmtKind::If {
                cond,
                then_body,
                elseifs,
                else_body,
            } => {
                let mut end_jumps = Vec::new();
                let b = self.emit_cond_bool(cond, depth)?;
                let mut miss = self.jump_if_false(b)?;
                self.emit_stmts(then_body, depth)?;
                if !elseifs.is_empty() || else_body.is_some() {
                    end_jumps.push(self.jump()?);
                }
                for (ec, eb) in elseifs {
                    let here = self.here();
                    self.patch(miss, here)?;
                    let cb = self.emit_cond_bool(ec, depth)?;
                    miss = self.jump_if_false(cb)?;
                    self.emit_stmts(eb, depth)?;
                    end_jumps.push(self.jump()?);
                }
                let else_at = self.here();
                self.patch(miss, else_at)?;
                if let Some(eb) = else_body {
                    self.emit_stmts(eb, depth)?;
                }
                let end = self.here();
                for j in end_jumps {
                    self.patch(j, end)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body } => {
                let head = self.here();
                let b = self.emit_cond_bool(cond, depth)?;
                let exit = self.jump_if_false(b)?;
                self.loops.push(LoopCtx {
                    breaks: vec![exit],
                    continues: Vec::new(),
                    handler_depth: self.handler_depth,
                });
                self.emit_stmts(body, depth)?;
                let ctx = self.loops.pop().unwrap();
                for cj in ctx.continues {
                    self.patch(cj, head)?;
                }
                let back = self.jump()?;
                self.patch(back, head)?;
                let end = self.here();
                for bj in ctx.breaks {
                    self.patch(bj, end)?;
                }
                Ok(())
            }
            StmtKind::DoWhile { body, cond } => {
                let head = self.here();
                self.loops.push(LoopCtx {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    handler_depth: self.handler_depth,
                });
                self.emit_stmts(body, depth)?;
                let ctx = self.loops.pop().unwrap();
                let cont_at = self.here();
                for cj in ctx.continues {
                    self.patch(cj, cont_at)?;
                }
                let b = self.emit_cond_bool(cond, depth)?;
                let back = self.jump_if_true(b)?;
                self.patch(back, head)?;
                let end = self.here();
                for bj in ctx.breaks {
                    self.patch(bj, end)?;
                }
                Ok(())
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                for e in init {
                    let m = self.mark();
                    self.emit_expr(e, depth)?;
                    self.release(m);
                }
                let head = self.here();
                let mut exit = None;
                if !cond.is_empty() {
                    for e in &cond[..cond.len() - 1] {
                        let m = self.mark();
                        self.emit_expr(e, depth)?;
                        self.release(m);
                    }
                    let b = self.emit_cond_bool(&cond[cond.len() - 1], depth)?;
                    exit = Some(self.jump_if_false(b)?);
                }
                self.loops.push(LoopCtx {
                    breaks: exit.into_iter().collect(),
                    continues: Vec::new(),
                    handler_depth: self.handler_depth,
                });
                self.emit_stmts(body, depth)?;
                let ctx = self.loops.pop().unwrap();
                let step_at = self.here();
                for cj in ctx.continues {
                    self.patch(cj, step_at)?;
                }
                for e in step {
                    let m = self.mark();
                    self.emit_expr(e, depth)?;
                    self.release(m);
                }
                let back = self.jump()?;
                self.patch(back, head)?;
                let end = self.here();
                for bj in ctx.breaks {
                    self.patch(bj, end)?;
                }
                Ok(())
            }
            StmtKind::ForEach {
                expr,
                key,
                by_ref,
                value,
                body,
            } => {
                let iter_target = if *by_ref {
                    self.emit_lvalue_ref(expr, depth)?
                } else {
                    self.emit_expr(expr, depth)?
                };
                let br = self.boolean(*by_ref)?;
                let it = self.helper("iterStart", &[iter_target, br])?;
                if !*by_ref {
                    // Fast inline zero-call loop for `kind === 4` (flattened PhpArray/Object snapshot),
                    // sharing a single emitted loop body with the generic `R.iterNext` path!
                    let kind = self.prop(it, "kind")?;
                    let k4 = self.small_int(4)?;
                    let is_fast = self.alloc()?;
                    self.emit(Instr::Eq {
                        dst: is_fast,
                        a: kind,
                        b: k4,
                    })?;
                    let keys_r = self.alloc()?;
                    let vals_r = self.alloc()?;
                    let len_r = self.alloc()?;
                    let idx_r = self.alloc()?;
                    let skip_init = self.jump_if_false(is_fast)?;
                    let kr = self.prop(it, "keys")?;
                    self.emit(Instr::Move { dst: keys_r, src: kr })?;
                    let vr = self.prop(it, "vals")?;
                    self.emit(Instr::Move { dst: vals_r, src: vr })?;
                    let lr = self.prop(it, "len")?;
                    self.emit(Instr::Move { dst: len_r, src: lr })?;
                    self.emit(Instr::LoadInt { dst: idx_r, val: 0 })?;
                    let head = self.here();
                    self.patch(skip_init, head)?;

                    let kv_r = self.alloc()?;
                    let vv_r = self.alloc()?;
                    let to_slow = self.jump_if_false(is_fast)?;
                    let exit_fast = self.emit(Instr::JumpIfNotLt {
                        a: idx_r,
                        b: len_r,
                        target: 0,
                    })?;
                    if key.is_some() {
                        self.emit(Instr::GetIndex {
                            dst: kv_r,
                            obj: keys_r,
                            key: idx_r,
                        })?;
                    }
                    self.emit(Instr::GetIndex {
                        dst: vv_r,
                        obj: vals_r,
                        key: idx_r,
                    })?;
                    self.emit(Instr::AddInt {
                        dst: idx_r,
                        a: idx_r,
                        imm: 1,
                        upd: true,
                    })?;
                    let go_body = self.jump()?;

                    let slow_at = self.here();
                    self.patch(to_slow, slow_at)?;
                    let has_next = self.helper("iterNext", &[it])?;
                    let exit_slow = self.jump_if_false(has_next)?;
                    if key.is_some() {
                        let kv = self.prop(it, "key")?;
                        self.emit(Instr::Move { dst: kv_r, src: kv })?;
                    }
                    let vv = self.prop(it, "val")?;
                    self.emit(Instr::Move { dst: vv_r, src: vv })?;

                    let body_at = self.here();
                    self.patch(go_body, body_at)?;
                    if let Some(kexpr) = key {
                        self.emit_assign_target_owned(kexpr, kv_r, depth)?;
                    }
                    self.emit_assign_target_owned(value, vv_r, depth)?;
                    self.loops.push(LoopCtx {
                        breaks: vec![exit_fast, exit_slow],
                        continues: Vec::new(),
                        handler_depth: self.handler_depth,
                    });
                    self.emit_stmts(body, depth)?;
                    let ctx = self.loops.pop().unwrap();
                    for cj in ctx.continues {
                        self.patch(cj, head)?;
                    }
                    let back = self.jump()?;
                    self.patch(back, head)?;
                    let end = self.here();
                    for bj in ctx.breaks {
                        self.patch(bj, end)?;
                    }
                    return Ok(());
                }
                let head = self.here();
                let has_next = self.helper("iterNext", &[it])?;
                let exit = self.jump_if_false(has_next)?;
                if let Some(kexpr) = key {
                    let kv = self.prop(it, "key")?;
                    self.emit_assign_target_owned(kexpr, kv, depth)?;
                }
                let vv = self.prop(it, "val")?;
                self.emit_assign_ref_target(value, vv, depth)?;
                self.loops.push(LoopCtx {
                    breaks: vec![exit],
                    continues: Vec::new(),
                    handler_depth: self.handler_depth,
                });
                self.emit_stmts(body, depth)?;
                let ctx = self.loops.pop().unwrap();
                for cj in ctx.continues {
                    self.patch(cj, head)?;
                }
                let back = self.jump()?;
                self.patch(back, head)?;
                let end = self.here();
                for bj in ctx.breaks {
                    self.patch(bj, end)?;
                }
                Ok(())
            }
            StmtKind::Switch { cond, cases } => {
                let subject = self.emit_expr(cond, depth)?;
                let mut case_jumps: Vec<(usize, usize)> = Vec::new();
                let mut default_idx: Option<usize> = None;
                for (i, c) in cases.iter().enumerate() {
                    if let Some(ce) = &c.cond {
                        let cv = self.emit_expr(ce, depth)?;
                        let eq = self.helper("eq", &[subject, cv])?;
                        let j = self.jump_if_true(eq)?;
                        case_jumps.push((j, i));
                    } else {
                        default_idx = Some(i);
                    }
                }
                let default_jump = self.jump()?;
                self.loops.push(LoopCtx {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    handler_depth: self.handler_depth,
                });
                let mut case_starts = Vec::with_capacity(cases.len());
                for c in cases {
                    case_starts.push(self.here());
                    self.emit_stmts(&c.body, depth)?;
                }
                let ctx = self.loops.pop().unwrap();
                let end = self.here();
                for (j, i) in case_jumps {
                    self.patch(j, case_starts[i])?;
                }
                if let Some(di) = default_idx {
                    self.patch(default_jump, case_starts[di])?;
                } else {
                    self.patch(default_jump, end)?;
                }
                for bj in ctx.breaks.into_iter().chain(ctx.continues) {
                    self.patch(bj, end)?;
                }
                Ok(())
            }
            StmtKind::Try {
                body,
                catches,
                finally,
            } => self.emit_try(body, catches, finally.as_deref(), depth),
            StmtKind::Namespace { name, body } => {
                if let Some(b) = body {
                    let prev = self.ns.clone();
                    self.ns.enter_namespace(name.as_deref());
                    self.emit_stmts_inner(b, depth, top_level)?;
                    self.ns = prev;
                } else {
                    self.ns.enter_namespace(name.as_deref());
                }
                Ok(())
            }
            StmtKind::Use(items) => {
                for it in items {
                    self.ns.add_use(it);
                }
                Ok(())
            }
            StmtKind::Declare { directives, body } => {
                for (k, v) in directives {
                    if k.eq_ignore_ascii_case("strict_types") {
                        if let ExprKind::Int(1) = v.kind {
                            self.file.strict_types = true;
                        }
                    }
                }
                if let Some(b) = body {
                    self.emit_stmts_inner(b, depth, top_level)?;
                }
                Ok(())
            }
            StmtKind::Const(items) => {
                for (name, expr) in items {
                    let fqn = self.ns.qualify_decl(name);
                    let k = self.string(&fqn)?;
                    let v = self.emit_expr(expr, depth)?;
                    self.helper("defConst", &[k, v])?;
                }
                Ok(())
            }
            StmtKind::Function(decl) => self.emit_function_decl(decl),
            StmtKind::Class(decl) => self.emit_class_decl(decl, depth),
            StmtKind::Interface(decl) => self.emit_interface_decl(decl, depth),
            StmtKind::Trait(decl) => self.emit_trait_decl(decl, depth),
            StmtKind::Enum(decl) => self.emit_enum_decl(decl, depth),
            StmtKind::Goto(name) => {
                let jmp_pc = self.jump()?;
                let hd = self.handler_depth;
                self.pending_gotos.push((name.clone(), jmp_pc, hd));
                Ok(())
            }
        }
    }

    fn const_break_level(&self, expr: Option<&Expr>) -> R<usize> {
        let Some(e) = expr else { return Ok(1) };
        match e.kind {
            ExprKind::Int(n) if n >= 1 => Ok(n as usize),
            _ => Err("PHP: break/continue level must be a positive integer constant".into()),
        }
    }

    fn emit_break_or_continue(&mut self, level: usize, is_continue: bool) -> R<()> {
        if level == 0 || level > self.loops.len() {
            return Err("PHP: break/continue level exceeds enclosing loop depth".into());
        }
        let target_idx = self.loops.len() - level;
        let floor = self.loops[target_idx].handler_depth;
        let j = if self.handler_depth > floor {
            self.emit(Instr::JumpFinally {
                target: 0,
                floor: floor as u16,
            })?
        } else {
            self.jump()?
        };
        if is_continue {
            self.loops[target_idx].continues.push(j);
        } else {
            self.loops[target_idx].breaks.push(j);
        }
        Ok(())
    }

    fn emit_try(
        &mut self,
        body: &[Stmt],
        catches: &[CatchClause],
        finally: Option<&[Stmt]>,
        depth: usize,
    ) -> R<()> {
        let has_fin = finally.is_some();
        let (kind_reg, val_reg, fin_push) = if has_fin {
            let kr = self.alloc()?;
            let vr = self.alloc()?;
            let at = self.emit(Instr::PushFinally {
                target: 0,
                kind_reg: kr,
                val_reg: vr,
            })?;
            self.handler_depth += 1;
            (kr, vr, Some(at))
        } else {
            (0, 0, None)
        };

        let mut to_finally = Vec::new();
        if !catches.is_empty() {
            let ereg = self.alloc()?;
            let push = self.emit(Instr::PushHandler {
                catch_target: 0,
                catch_reg: ereg,
            })?;
            self.handler_depth += 1;
            self.emit_stmts(body, depth)?;
            self.emit(Instr::PopHandler)?;
            self.handler_depth -= 1;
            if has_fin {
                self.emit(Instr::LoadInt {
                    dst: kind_reg,
                    val: 0,
                })?;
                self.emit(Instr::PopFinally)?;
            }
            to_finally.push(self.jump()?);

            let catch_start = self.here();
            self.patch(push, catch_start)?;
            let scope_cls = self.scope_cls_reg()?;
            for clause in catches {
                let mut type_regs = Vec::with_capacity(clause.types.len());
                for t in &clause.types {
                    let fqn = self.ns.resolve_class(t);
                    type_regs.push(self.string(&fqn)?);
                }
                let tarr = self.js_array(&type_regs)?;
                let matched = self.helper("catchMatch", &[ereg, tarr, scope_cls])?;
                let next_clause = self.jump_if_false(matched)?;
                if let Some(vname) = &clause.var {
                    self.store_var(vname, ereg)?;
                }
                self.emit_stmts(&clause.body, depth)?;
                if has_fin {
                    self.emit(Instr::LoadInt {
                        dst: kind_reg,
                        val: 0,
                    })?;
                    self.emit(Instr::PopFinally)?;
                }
                to_finally.push(self.jump()?);
                let after = self.here();
                self.patch(next_clause, after)?;
            }
            // No catch matched -> rethrow!
            self.emit(Instr::Throw { src: ereg })?;
        } else {
            self.emit_stmts(body, depth)?;
            if has_fin {
                self.emit(Instr::LoadInt {
                    dst: kind_reg,
                    val: 0,
                })?;
                self.emit(Instr::PopFinally)?;
            }
            to_finally.push(self.jump()?);
        }

        if let Some(fin_stmts) = finally {
            self.handler_depth -= 1;
            let fin_start = self.here();
            if let Some(fp) = fin_push {
                self.patch(fp, fin_start)?;
            }
            for j in to_finally {
                self.patch(j, fin_start)?;
            }
            self.emit_stmts(fin_stmts, depth)?;
            self.emit(Instr::EndFinally { kind_reg, val_reg })?;
        } else {
            let end = self.here();
            for j in to_finally {
                self.patch(j, end)?;
            }
        }
        Ok(())
    }

    // ---- Function / Method / Closure compilation ----
    pub fn compile_php_function(
        &mut self,
        display_name: &str,
        params: &[Param],
        uses: &[ClosureUse],
        body: &[Stmt],
        class_name: Option<String>,
        trait_name: Option<String>,
        fn_name: String,
        promoted_props: &[(String, bool)],
    ) -> R<(u32, bool, bool, bool)> {
        let info = analyse_body(body, params, uses, false);
        let is_gen = info.is_generator;
        let has_func_args = info.uses_func_args;
        let is_direct = !info.use_scope_map
            && !is_gen
            && !has_func_args
            && params.len() <= 2
            && params
                .iter()
                .all(|p| !p.by_ref && !p.variadic && !is_superglobal(&p.name));
        let mut child = PhpEmitter::new(
            display_name,
            self.rt_slot,
            self.sink,
            self.file.clone(),
            self.ns.clone(),
            class_name,
            trait_name,
            fn_name,
            &info,
            params,
            uses,
            false,
            is_direct,
        )?;
        if child.class_name.is_some() && child.trait_name.is_none() {
            child.class_fast_props = self.class_fast_props.clone();
            child.class_private_methods = self.class_private_methods.clone();
        }
        // Constructor property promotion: `$this->prop = $prop;`
        if !promoted_props.is_empty() {
            let scope_cls = child.scope_cls_reg()?;
            for (pname, _) in promoted_props {
                let mark = child.mark();
                let pv = child.load_var(pname, true)?;
                let pn = child.string(pname)?;
                child.helper("propSet", &[REG_THIS, pn, pv, scope_cls])?;
                child.release(mark);
            }
        }
        child.emit_stmts(body, 0)?;
        let ret = child.null()?;
        child.emit(Instr::Return { src: ret })?;
        child.patch_gotos()?;
        let proto = child.finish();
        let func_id = self.sink.push(proto)?;
        Ok((func_id, is_gen, has_func_args, is_direct))
    }

    pub fn emit_default_thunk(&mut self, expr: &Expr) -> R<Reg> {
        let info = super::symtable::FnScopeInfo::default();
        let mut child = PhpEmitter::new(
            "<default>",
            self.rt_slot,
            self.sink,
            self.file.clone(),
            self.ns.clone(),
            self.class_name.clone(),
            self.trait_name.clone(),
            self.fn_name.clone(),
            &info,
            &[],
            &[],
            false,
            false,
        )?;
        let v = child.emit_expr(expr, 0)?;
        child.emit(Instr::Return { src: v })?;
        let fid = self.sink.push(child.finish())?;
        let dst = self.alloc()?;
        self.emit(Instr::MakeFunc { dst, func_id: fid })?;
        Ok(dst)
    }

    pub fn emit_params_meta(&mut self, params: &[Param]) -> R<Reg> {
        let mut regs = Vec::with_capacity(params.len());
        for p in params {
            let obj = self.new_js_obj()?;
            let n = self.string(&p.name)?;
            self.set_prop(obj, "name", n)?;
            if p.by_ref {
                let br = self.boolean(true)?;
                self.set_prop(obj, "byRef", br)?;
            }
            if p.variadic {
                let vr = self.boolean(true)?;
                self.set_prop(obj, "variadic", vr)?;
            }
            if p.visibility.is_some() || p.readonly {
                let pr = self.boolean(true)?;
                self.set_prop(obj, "promoted", pr)?;
            }
            if let Some(ty) = &p.ty {
                let ts = self.ns.format_type(ty);
                let tr = self.string(&ts)?;
                self.set_prop(obj, "ty", tr)?;
            }
            if !p.attributes.is_empty() {
                let ar = self.emit_attrs_meta(&p.attributes, 0)?;
                self.set_prop(obj, "attrs", ar)?;
            }
            if let Some(def) = &p.default {
                let hd = self.boolean(true)?;
                self.set_prop(obj, "hasDefault", hd)?;
                // Inline literal defaults directly as `defaultVal` instead of allocating a thunk FuncProto!
                match &def.kind {
                    ExprKind::Null => {
                        let v = self.null()?;
                        self.set_prop(obj, "defaultVal", v)?;
                    }
                    ExprKind::Bool(b) => {
                        let v = self.boolean(*b)?;
                        self.set_prop(obj, "defaultVal", v)?;
                    }
                    ExprKind::Int(i) => {
                        let v = self.int64(*i)?;
                        self.set_prop(obj, "defaultVal", v)?;
                    }
                    ExprKind::Float(f) => {
                        let v = self.float(*f)?;
                        self.set_prop(obj, "defaultVal", v)?;
                    }
                    ExprKind::String(s) => {
                        let v = self.string(s)?;
                        self.set_prop(obj, "defaultVal", v)?;
                    }
                    _ => {
                        let dfn = self.emit_default_thunk(def)?;
                        self.set_prop(obj, "defaultFn", dfn)?;
                    }
                }
            }
            regs.push(obj);
        }
        self.js_array(&regs)
    }

    pub fn emit_attrs_meta(&mut self, attrs: &[Attribute], depth: usize) -> R<Reg> {
        let mut regs = Vec::with_capacity(attrs.len());
        for a in attrs {
            let obj = self.new_js_obj()?;
            let fqn = self.ns.resolve_class(&a.name);
            let nr = self.string(&fqn)?;
            self.set_prop(obj, "name", nr)?;
            let arr = self.helper("arr", &[])?;
            for arg in &a.args {
                let v = self.emit_expr(&arg.value, depth)?;
                if let Some(k) = &arg.name {
                    let kr = self.string(k)?;
                    self.helper("arrSetLit", &[arr, kr, v])?;
                } else {
                    self.helper("arrPush", &[arr, v])?;
                }
            }
            self.set_prop(obj, "args", arr)?;
            regs.push(obj);
        }
        self.js_array(&regs)
    }

    fn emit_function_decl(&mut self, decl: &FunctionDecl) -> R<()> {
        let fqn = self.ns.qualify_decl(&decl.name);
        let (fid, is_gen, has_func_args, is_direct) = self.compile_php_function(
            &fqn,
            &decl.params,
            &[],
            &decl.body,
            None,
            None,
            fqn.clone(),
            &[],
        )?;
        let code = self.alloc()?;
        self.emit(Instr::MakeFunc {
            dst: code,
            func_id: fid,
        })?;
        let meta = self.new_js_obj()?;
        let params = self.emit_params_meta(&decl.params)?;
        self.set_prop(meta, "params", params)?;
        if decl.by_ref {
            let br = self.boolean(true)?;
            self.set_prop(meta, "byRef", br)?;
        }
        if is_gen {
            let ig = self.boolean(true)?;
            self.set_prop(meta, "isGen", ig)?;
        }
        if has_func_args {
            let hfa = self.boolean(true)?;
            self.set_prop(meta, "hasFuncArgs", hfa)?;
        }
        if is_direct {
            let dr = self.boolean(true)?;
            self.set_prop(meta, "direct", dr)?;
        }
        if self.file.strict_types {
            let st = self.boolean(true)?;
            self.set_prop(meta, "strict", st)?;
        }
        if let Some(rt) = &decl.return_type {
            let s = self.ns.format_type(rt);
            let r = self.string(&s)?;
            self.set_prop(meta, "retType", r)?;
        }
        let fi_info = analyse_body(&decl.body, &decl.params, &[], false);
        if let Some(fi) = super::fast_int::try_compile_fast_int(
            &fqn,
            &self.ns,
            decl,
            &fi_info,
            self.sink,
        )? {
            let fi_fn = self.alloc()?;
            if let Some(slot) = fi.self_global_slot {
                // `hoist_functions()` already populated `globals[slot]` with `HeapObj::Func(fi.func_id)`;
                // loading from it keeps the canonical hoisted function identity for Tier-A self-calls!
                self.emit(Instr::LoadGlobal {
                    dst: fi_fn,
                    idx: slot,
                })?;
            } else {
                self.emit(Instr::MakeFunc {
                    dst: fi_fn,
                    func_id: fi.func_id,
                })?;
            }
            self.set_prop(meta, "fastInt", fi_fn)?;
            let b_reg = self.int64(fi.bound)?;
            self.set_prop(meta, "fiBound", b_reg)?;
        }
        let fqn_r = self.string(&fqn)?;
        self.helper("defFn", &[fqn_r, code, meta])?;
        Ok(())
    }

    pub fn emit_class_spec(
        &mut self,
        kind: &str,
        fqn: &str,
        is_abstract: bool,
        is_final: bool,
        is_readonly: bool,
        backing_type: Option<&str>,
        extends: Option<&Name>,
        interface_extends: &[Name],
        implements: &[Name],
        cases: &[EnumCase],
        members: &[ClassMember],
        attrs: &[Attribute],
        depth: usize,
    ) -> R<Reg> {
        let prev_class = self.class_name.replace(fqn.to_owned());
        let mut fast_props = std::collections::HashMap::new();
        if kind == "class" && !is_readonly {
            let map_simple_ty = |ty: &Option<TypeHint>| -> Option<Option<u8>> {
                match ty {
                    None => Some(None),
                    Some(TypeHint::Simple(n)) if n.parts.len() == 1 => {
                        match n.parts[0].to_ascii_lowercase().as_str() {
                            "int" => Some(Some(7)),
                            "string" => Some(Some(1)),
                            "bool" => Some(Some(2)),
                            "float" => Some(Some(0)),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            };
            for m in members {
                match m {
                    ClassMember::Property {
                        visibility,
                        set_visibility,
                        is_static,
                        is_readonly: p_ro,
                        ty,
                        items,
                        hooks,
                        ..
                    } => {
                        if !*is_static
                            && !*p_ro
                            && *visibility != Visibility::Private
                            && *set_visibility != Some(Visibility::Private)
                            && hooks.is_empty()
                        {
                            if let Some(code) = map_simple_ty(ty) {
                                for it in items {
                                    fast_props.insert(it.0.clone(), code);
                                }
                            }
                        }
                    }
                    ClassMember::Method { name, params, .. }
                        if name.eq_ignore_ascii_case("__construct") =>
                    {
                        for p in params {
                            if let Some(vis) = p.visibility {
                                if !p.readonly
                                    && vis != Visibility::Private
                                    && p.set_visibility != Some(Visibility::Private)
                                {
                                    if let Some(code) = map_simple_ty(&p.ty) {
                                        fast_props.insert(p.name.clone(), code);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut private_methods = std::collections::HashSet::new();
        for m in members {
            if let ClassMember::Method {
                visibility: Visibility::Private,
                name,
                ..
            } = m
            {
                private_methods.insert(name.to_ascii_lowercase());
            }
        }
        let prev_fast_props = std::mem::replace(&mut self.class_fast_props, fast_props);
        let prev_priv_methods = std::mem::replace(&mut self.class_private_methods, private_methods);
        let res = self.emit_class_spec_body(
            kind,
            fqn,
            is_abstract,
            is_final,
            is_readonly,
            backing_type,
            extends,
            interface_extends,
            implements,
            cases,
            members,
            attrs,
            depth,
        );
        self.class_name = prev_class;
        self.class_fast_props = prev_fast_props;
        self.class_private_methods = prev_priv_methods;
        res
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_class_spec_body(
        &mut self,
        kind: &str,
        fqn: &str,
        is_abstract: bool,
        is_final: bool,
        is_readonly: bool,
        backing_type: Option<&str>,
        extends: Option<&Name>,
        interface_extends: &[Name],
        implements: &[Name],
        cases: &[EnumCase],
        members: &[ClassMember],
        attrs: &[Attribute],
        depth: usize,
    ) -> R<Reg> {
        let spec = self.new_js_obj()?;
        let kr = self.string(kind)?;
        self.set_prop(spec, "kind", kr)?;
        let nr = self.string(fqn)?;
        self.set_prop(spec, "name", nr)?;
        let fname = self.file.file.clone();
        let fr = self.string(&fname)?;
        self.set_prop(spec, "file", fr)?;
        if is_abstract {
            let v = self.boolean(true)?;
            self.set_prop(spec, "isAbstract", v)?;
        }
        if is_final {
            let v = self.boolean(true)?;
            self.set_prop(spec, "isFinal", v)?;
        }
        if is_readonly {
            let v = self.boolean(true)?;
            self.set_prop(spec, "isReadonly", v)?;
        }
        if let Some(bt) = backing_type {
            let v = self.string(bt)?;
            self.set_prop(spec, "backingType", v)?;
        }
        if let Some(ex) = extends {
            let s = self.ns.resolve_class(ex);
            let v = self.string(&s)?;
            self.set_prop(spec, "extends", v)?;
        } else if !interface_extends.is_empty() {
            let mut ex_regs = Vec::new();
            for ex in interface_extends {
                let s = self.ns.resolve_class(ex);
                ex_regs.push(self.string(&s)?);
            }
            let arr = self.js_array(&ex_regs)?;
            self.set_prop(spec, "extends", arr)?;
        }
        if !implements.is_empty() {
            let mut im_regs = Vec::new();
            for im in implements {
                let s = self.ns.resolve_class(im);
                im_regs.push(self.string(&s)?);
            }
            let arr = self.js_array(&im_regs)?;
            self.set_prop(spec, "implements", arr)?;
        }
        if !attrs.is_empty() {
            let ar = self.emit_attrs_meta(attrs, depth)?;
            self.set_prop(spec, "attrs", ar)?;
        }

        let mut trait_regs = Vec::new();
        let mut const_regs = Vec::new();
        let mut prop_regs = Vec::new();
        let mut method_regs = Vec::new();

        for m in members {
            match m {
                ClassMember::TraitUse {
                    traits,
                    adaptations,
                    ..
                } => {
                    let tu = self.new_js_obj()?;
                    let mut tnames = Vec::new();
                    for t in traits {
                        let s = self.ns.resolve_class(t);
                        tnames.push(self.string(&s)?);
                    }
                    let ta = self.js_array(&tnames)?;
                    self.set_prop(tu, "names", ta)?;
                    if !adaptations.is_empty() {
                        let mut ad_regs = Vec::new();
                        for ad in adaptations {
                            let ao = self.new_js_obj()?;
                            match ad {
                                TraitAdaptation::Precedence {
                                    trait_name,
                                    method,
                                    instead_of,
                                } => {
                                    let k = self.string("precedence")?;
                                    self.set_prop(ao, "kind", k)?;
                                    if let Some(tn) = trait_name {
                                        let s = self.ns.resolve_class(tn);
                                        let r = self.string(&s)?;
                                        self.set_prop(ao, "trait", r)?;
                                    }
                                    let mr = self.string(method)?;
                                    self.set_prop(ao, "method", mr)?;
                                    let mut io = Vec::new();
                                    for x in instead_of {
                                        let s = self.ns.resolve_class(x);
                                        io.push(self.string(&s)?);
                                    }
                                    let ioa = self.js_array(&io)?;
                                    self.set_prop(ao, "insteadOf", ioa)?;
                                }
                                TraitAdaptation::Alias {
                                    trait_name,
                                    method,
                                    visibility,
                                    alias,
                                } => {
                                    let k = self.string("alias")?;
                                    self.set_prop(ao, "kind", k)?;
                                    if let Some(tn) = trait_name {
                                        let s = self.ns.resolve_class(tn);
                                        let r = self.string(&s)?;
                                        self.set_prop(ao, "trait", r)?;
                                    }
                                    let mr = self.string(method)?;
                                    self.set_prop(ao, "method", mr)?;
                                    if let Some(vis) = visibility {
                                        let vr = self.small_int(vis_code(*vis))?;
                                        self.set_prop(ao, "vis", vr)?;
                                    }
                                    if let Some(al) = alias {
                                        let ar = self.string(al)?;
                                        self.set_prop(ao, "alias", ar)?;
                                    }
                                }
                            }
                            ad_regs.push(ao);
                        }
                        let ada = self.js_array(&ad_regs)?;
                        self.set_prop(tu, "adaptations", ada)?;
                    }
                    trait_regs.push(tu);
                }
                ClassMember::Const {
                    visibility, items, ..
                } => {
                    for (cname, val_expr) in items {
                        let co = self.new_js_obj()?;
                        let cn = self.string(cname)?;
                        self.set_prop(co, "name", cn)?;
                        let vr = self.small_int(vis_code(*visibility))?;
                        self.set_prop(co, "vis", vr)?;
                        if is_simple_literal(val_expr) {
                            let vv = self.emit_expr(val_expr, depth)?;
                            self.set_prop(co, "val", vv)?;
                        } else {
                            let vf = self.emit_default_thunk(val_expr)?;
                            self.set_prop(co, "valFn", vf)?;
                        }
                        const_regs.push(co);
                    }
                }
                ClassMember::Property {
                    attributes: pattrs,
                    visibility,
                    set_visibility,
                    is_static,
                    is_readonly: pro,
                    ty,
                    items,
                    hooks,
                    ..
                } => {
                    for (pname, init) in items {
                        let po = self.new_js_obj()?;
                        let pn = self.string(pname)?;
                        self.set_prop(po, "name", pn)?;
                        let vr = self.small_int(vis_code(*visibility))?;
                        self.set_prop(po, "vis", vr)?;
                        if let Some(sv) = set_visibility {
                            let svr = self.small_int(vis_code(*sv))?;
                            self.set_prop(po, "setVis", svr)?;
                        }
                        if *is_static {
                            let s = self.boolean(true)?;
                            self.set_prop(po, "isStatic", s)?;
                        }
                        if *pro {
                            let r = self.boolean(true)?;
                            self.set_prop(po, "readonly", r)?;
                        }
                        if let Some(t) = ty {
                            let ts = self.ns.format_type(t);
                            let tr = self.string(&ts)?;
                            self.set_prop(po, "ty", tr)?;
                        }
                        if let Some(ie) = init {
                            let hd = self.boolean(true)?;
                            self.set_prop(po, "hasDefault", hd)?;
                            if is_simple_literal(ie) {
                                let dv = self.emit_expr(ie, depth)?;
                                self.set_prop(po, "defaultVal", dv)?;
                            } else {
                                let df = self.emit_default_thunk(ie)?;
                                self.set_prop(po, "defaultFn", df)?;
                            }
                        } else if ty.is_none() && hooks.is_empty() {
                            // Untyped PHP properties default to `null`
                            let hd = self.boolean(true)?;
                            self.set_prop(po, "hasDefault", hd)?;
                            let dv = self.null()?;
                            self.set_prop(po, "defaultVal", dv)?;
                        }
                        // PHP 8.4 property hooks (`get` / `set`)
                        for hook in hooks {
                            let cls_arg = if kind == "trait" { None } else { Some(fqn.to_owned()) };
                            let tr_arg = if kind == "trait" { Some(fqn.to_owned()) } else { None };
                            match hook.kind {
                                PropHookKind::Get => {
                                    let stmts = match &hook.body {
                                        PropHookBody::Expr(e) => vec![Stmt {
                                            kind: StmtKind::Return(Some(e.clone())),
                                            span: e.span,
                                        }],
                                        PropHookBody::Block(b) => b.clone(),
                                        PropHookBody::Abstract => continue,
                                    };
                                    let disp = format!("{fqn}::${pname}::get");
                                    let (fid, _, _, is_direct) = self.compile_php_function(
                                        &disp, &[], &[], &stmts, cls_arg, tr_arg, disp.clone(), &[],
                                    )?;
                                    let code = self.alloc()?;
                                    self.emit(Instr::MakeFunc { dst: code, func_id: fid })?;
                                    self.set_prop(po, "getHook", code)?;
                                    if is_direct {
                                        let dr = self.boolean(true)?;
                                        self.set_prop(po, "getHookDirect", dr)?;
                                    }
                                }
                                PropHookKind::Set => {
                                    let (stmts, is_arrow) = match &hook.body {
                                        PropHookBody::Expr(e) => (
                                            vec![Stmt {
                                                kind: StmtKind::Return(Some(e.clone())),
                                                span: e.span,
                                            }],
                                            true,
                                        ),
                                        PropHookBody::Block(b) => (b.clone(), false),
                                        PropHookBody::Abstract => continue,
                                    };
                                    let set_param = hook.param.clone().unwrap_or(Param {
                                        attributes: Vec::new(),
                                        visibility: None,
                                        set_visibility: None,
                                        readonly: false,
                                        ty: ty.clone(),
                                        by_ref: false,
                                        variadic: false,
                                        name: "value".to_owned(),
                                        default: None,
                                        span: hook.span,
                                    });
                                    let disp = format!("{fqn}::${pname}::set");
                                    let (fid, _, _, is_direct) = self.compile_php_function(
                                        &disp, &[set_param], &[], &stmts, cls_arg, tr_arg, disp.clone(), &[],
                                    )?;
                                    let code = self.alloc()?;
                                    self.emit(Instr::MakeFunc { dst: code, func_id: fid })?;
                                    self.set_prop(po, "setHook", code)?;
                                    if is_direct {
                                        let dr = self.boolean(true)?;
                                        self.set_prop(po, "setHookDirect", dr)?;
                                    }
                                    if is_arrow {
                                        let ia = self.boolean(true)?;
                                        self.set_prop(po, "setHookIsArrow", ia)?;
                                    }
                                }
                            }
                        }
                        if !pattrs.is_empty() {
                            let ar = self.emit_attrs_meta(pattrs, depth)?;
                            self.set_prop(po, "attrs", ar)?;
                        }
                        prop_regs.push(po);
                    }
                }
                ClassMember::Method {
                    attributes: mattrs,
                    visibility,
                    is_static,
                    is_abstract: mab,
                    is_final: mfin,
                    by_ref,
                    name,
                    params,
                    return_type,
                    body,
                    ..
                } => {
                    // Constructor property promotion
                    let mut promoted = Vec::new();
                    if name.eq_ignore_ascii_case("__construct") {
                        for p in params {
                            if p.visibility.is_some() || p.readonly {
                                let po = self.new_js_obj()?;
                                let pn = self.string(&p.name)?;
                                self.set_prop(po, "name", pn)?;
                                let vr = self
                                    .small_int(vis_code(p.visibility.unwrap_or(Visibility::Public)))?;
                                self.set_prop(po, "vis", vr)?;
                                if let Some(sv) = p.set_visibility {
                                    let svr = self.small_int(vis_code(sv))?;
                                    self.set_prop(po, "setVis", svr)?;
                                }
                                if p.readonly {
                                    let r = self.boolean(true)?;
                                    self.set_prop(po, "readonly", r)?;
                                }
                                let pr = self.boolean(true)?;
                                self.set_prop(po, "promoted", pr)?;
                                if let Some(t) = &p.ty {
                                    let ts = self.ns.format_type(t);
                                    let tr = self.string(&ts)?;
                                    self.set_prop(po, "ty", tr)?;
                                }
                                prop_regs.push(po);
                                promoted.push((p.name.clone(), p.readonly));
                            }
                        }
                    }

                    let mo = self.new_js_obj()?;
                    let mn = self.string(name)?;
                    self.set_prop(mo, "name", mn)?;
                    let vr = self.small_int(vis_code(*visibility))?;
                    self.set_prop(mo, "vis", vr)?;
                    if *is_static {
                        let v = self.boolean(true)?;
                        self.set_prop(mo, "isStatic", v)?;
                    }
                    if *mab {
                        let v = self.boolean(true)?;
                        self.set_prop(mo, "isAbstract", v)?;
                    }
                    if *mfin {
                        let v = self.boolean(true)?;
                        self.set_prop(mo, "isFinal", v)?;
                    }
                    let br = self.boolean(*by_ref)?;
                    self.set_prop(mo, "byRef", br)?;
                    let st = self.boolean(self.file.strict_types)?;
                    self.set_prop(mo, "strict", st)?;
                    let pm = self.emit_params_meta(params)?;
                    self.set_prop(mo, "params", pm)?;
                    if let Some(rt) = return_type {
                        let ts = self.ns.format_type(rt);
                        let tr = self.string(&ts)?;
                        self.set_prop(mo, "retType", tr)?;
                    }
                    if !mattrs.is_empty() {
                        let ar = self.emit_attrs_meta(mattrs, depth)?;
                        self.set_prop(mo, "attrs", ar)?;
                    }
                    if let Some(mbody) = body {
                        let disp = format!("{fqn}::{name}");
                        let cls_arg = if kind == "trait" {
                            None
                        } else {
                            Some(fqn.to_owned())
                        };
                        let tr_arg = if kind == "trait" {
                            Some(fqn.to_owned())
                        } else {
                            None
                        };
                        let (fid, is_gen, has_func_args, is_direct) = self.compile_php_function(
                            &disp,
                            params,
                            &[],
                            mbody,
                            cls_arg,
                            tr_arg,
                            name.clone(),
                            &promoted,
                        )?;
                        let code = self.alloc()?;
                        self.emit(Instr::MakeFunc {
                            dst: code,
                            func_id: fid,
                        })?;
                        self.set_prop(mo, "code", code)?;
                        if is_gen {
                            let g = self.boolean(true)?;
                            self.set_prop(mo, "isGen", g)?;
                        }
                        if has_func_args {
                            let hfa = self.boolean(true)?;
                            self.set_prop(mo, "hasFuncArgs", hfa)?;
                        }
                        if is_direct {
                            let dr = self.boolean(true)?;
                            self.set_prop(mo, "direct", dr)?;
                        }
                    }
                    method_regs.push(mo);
                }
            }
        }

        if !cases.is_empty() {
            let mut case_regs = Vec::new();
            for ec in cases {
                let co = self.new_js_obj()?;
                let cn = self.string(&ec.name)?;
                self.set_prop(co, "name", cn)?;
                if let Some(ve) = &ec.value {
                    let v = self.emit_expr(ve, depth)?;
                    self.set_prop(co, "val", v)?;
                }
                case_regs.push(co);
            }
            let ca = self.js_array(&case_regs)?;
            self.set_prop(spec, "cases", ca)?;
        }
        if !trait_regs.is_empty() {
            let a = self.js_array(&trait_regs)?;
            self.set_prop(spec, "traits", a)?;
        }
        if !const_regs.is_empty() {
            let a = self.js_array(&const_regs)?;
            self.set_prop(spec, "consts", a)?;
        }
        if !prop_regs.is_empty() {
            let a = self.js_array(&prop_regs)?;
            self.set_prop(spec, "props", a)?;
        }
        if !method_regs.is_empty() {
            let a = self.js_array(&method_regs)?;
            self.set_prop(spec, "methods", a)?;
        }

        Ok(spec)
    }

    fn emit_class_decl(&mut self, decl: &ClassDecl, depth: usize) -> R<()> {
        let fqn = self.ns.qualify_decl(&decl.name);
        let spec = self.emit_class_spec(
            "class",
            &fqn,
            decl.is_abstract,
            decl.is_final,
            decl.is_readonly,
            None,
            decl.extends.as_ref(),
            &[],
            &decl.implements,
            &[],
            &decl.members,
            &decl.attributes,
            depth,
        )?;
        self.helper("defClass", &[spec])?;
        Ok(())
    }

    fn emit_interface_decl(&mut self, decl: &InterfaceDecl, depth: usize) -> R<()> {
        let fqn = self.ns.qualify_decl(&decl.name);
        let spec = self.emit_class_spec(
            "interface",
            &fqn,
            false,
            false,
            false,
            None,
            None,
            &decl.extends,
            &[],
            &[],
            &decl.members,
            &decl.attributes,
            depth,
        )?;
        self.helper("defInterface", &[spec])?;
        Ok(())
    }

    fn emit_trait_decl(&mut self, decl: &TraitDecl, depth: usize) -> R<()> {
        let fqn = self.ns.qualify_decl(&decl.name);
        let spec = self.emit_class_spec(
            "trait",
            &fqn,
            false,
            false,
            false,
            None,
            None,
            &[],
            &[],
            &[],
            &decl.members,
            &decl.attributes,
            depth,
        )?;
        self.helper("defTrait", &[spec])?;
        Ok(())
    }

    fn emit_enum_decl(&mut self, decl: &EnumDecl, depth: usize) -> R<()> {
        let fqn = self.ns.qualify_decl(&decl.name);
        let spec = self.emit_class_spec(
            "enum",
            &fqn,
            false,
            true,
            false,
            decl.backing_type.as_deref(),
            None,
            &[],
            &decl.implements,
            &decl.cases,
            &decl.members,
            &decl.attributes,
            depth,
        )?;
        self.helper("defEnum", &[spec])?;
        Ok(())
    }

    pub fn emit_unset_target(&mut self, expr: &Expr, depth: usize) -> R<()> {
        match &expr.kind {
            ExprKind::Var(name) => self.unset_var(name),
            ExprKind::DynamicVar(inner) => {
                let n = self.emit_expr(inner, depth)?;
                let scope = match self.r_scope {
                    Some(r) => r,
                    None => self.null()?,
                };
                self.helper("sunset", &[scope, n])?;
                Ok(())
            }
            ExprKind::Index { .. } => {
                let (root, keys) = peel_index_chain(expr);
                let root_val = self.emit_expr(root, depth)?;
                let updated = if keys.len() == 1 {
                    let Some(ke) = keys[0] else {
                        return Err("PHP: cannot unset with empty index []".into());
                    };
                    let k = self.emit_expr(ke, depth)?;
                    self.helper("idxUnset", &[root_val, k])?
                } else {
                    let mut kregs = Vec::with_capacity(keys.len());
                    for k in keys {
                        match k {
                            Some(ke) => kregs.push(self.emit_expr(ke, depth)?),
                            None => return Err("PHP: cannot unset with empty index []".into()),
                        }
                    }
                    let karr = self.js_array(&kregs)?;
                    self.helper("idxUnsetPath", &[root_val, karr])?
                };
                if matches!(
                    root.kind,
                    ExprKind::Var(_)
                        | ExprKind::DynamicVar(_)
                        | ExprKind::Prop { .. }
                        | ExprKind::StaticProp { .. }
                ) {
                    self.emit_assign_target_owned(root, updated, depth)?;
                }
                Ok(())
            }
            ExprKind::Prop { target, prop, .. } => {
                let obj = self.emit_expr(target, depth)?;
                let pname = self.emit_prop_name(prop, depth)?;
                let sc = self.scope_cls_reg()?;
                self.helper("propUnset", &[obj, pname, sc])?;
                Ok(())
            }
            _ => Err("PHP: invalid unset() target".into()),
        }
    }
}

fn is_simple_literal(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Null
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
    )
}

fn vis_code(v: Visibility) -> i32 {
    match v {
        Visibility::Public => 0,
        Visibility::Protected => 1,
        Visibility::Private => 2,
    }
}

/// Peel nested `$root[$k0][$k1]...` into `(root_expr, [k0, k1, ...])` in outer-to-inner order.
pub(super) fn peel_index_chain(mut expr: &Expr) -> (&Expr, Vec<Option<&Expr>>) {
    let mut keys = Vec::new();
    while let ExprKind::Index { target, index } = &expr.kind {
        keys.push(index.as_deref());
        expr = target;
    }
    keys.reverse();
    (expr, keys)
}

#[allow(dead_code)]
fn _keep_imports() {
    let _ = (REG_CALLED, REG_SCOPE, is_superglobal);
}
