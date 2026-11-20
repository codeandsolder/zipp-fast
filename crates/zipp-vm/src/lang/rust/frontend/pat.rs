//! Pattern compilation (irrefutable bindings for `let`/params/`for` and refutable
//! matching for `match`, `if let`, `while let`, and `let ... else`).

use super::emitter::{RsEmitter, R};
use super::scope::NumKind;
use crate::bytecode::{Instr, Reg};
use zipp_rsparse::ast::*;

impl<'a> RsEmitter<'a> {
    /// Irrefutable pattern binding (`let`, function params, closure params, `for`).
    pub(super) fn emit_pat_bind(&mut self, pat: &Pat, val: Reg, num_kind: NumKind) -> R<()> {
        match &pat.kind {
            PatKind::Wildcard | PatKind::Rest => Ok(()),
            PatKind::Ident { name, subpat, .. } => {
                self.bind_local(name, val, num_kind)?;
                if let Some(sub) = subpat {
                    self.emit_pat_bind(sub, val, num_kind)?;
                }
                Ok(())
            }
            PatKind::Ref { inner, .. } => {
                let d = self.helper("deref", &[val])?;
                self.emit_pat_bind(inner, d, num_kind)
            }
            PatKind::Tuple(elems) => {
                let rest_pos = elems.iter().position(|p| matches!(p.kind, PatKind::Rest));
                match rest_pos {
                    None => {
                        for (i, p) in elems.iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("getTupleIdx", &[val, idx])?;
                            self.emit_pat_bind(p, item, NumKind::Unknown)?;
                        }
                    }
                    Some(rp) => {
                        for (i, p) in elems[..rp].iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("getTupleIdx", &[val, idx])?;
                            self.emit_pat_bind(p, item, NumKind::Unknown)?;
                        }
                        let tail_count = elems.len() - rp - 1;
                        if tail_count > 0 {
                            let total = self.helper("patSliceLen", &[val])?;
                            for (j, p) in elems[rp + 1..].iter().enumerate() {
                                let off = self.small_int((tail_count - j) as i32)?;
                                let idx = self.alloc()?;
                                self.emit(Instr::Sub {
                                    dst: idx,
                                    a: total,
                                    b: off,
                                })?;
                                let item = self.helper("getTupleIdx", &[val, idx])?;
                                self.emit_pat_bind(p, item, NumKind::Unknown)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            PatKind::Slice(elems) => {
                self.emit_slice_bind(elems, val)
            }
            PatKind::TupleStruct { elems, .. } => {
                for (i, p) in elems.iter().enumerate() {
                    if matches!(p.kind, PatKind::Rest) {
                        break;
                    }
                    let idx = self.small_int(i as i32)?;
                    let item = self.helper("getTupleIdx", &[val, idx])?;
                    self.emit_pat_bind(p, item, NumKind::Unknown)?;
                }
                Ok(())
            }
            PatKind::Struct { fields, .. } => {
                for f in fields {
                    let fname = self.string(&f.name)?;
                    let fval = self.helper("getField", &[val, fname])?;
                    self.emit_pat_bind(&f.pat, fval, NumKind::Unknown)?;
                }
                Ok(())
            }
            PatKind::Path(p) => {
                // Single uppercase ident in irrefutable context (e.g. `let X = ...;`) binds as local!
                if p.segments.len() == 1 && !p.leading_colon {
                    self.bind_local(&p.segments[0].ident, val, num_kind)?;
                }
                Ok(())
            }
            PatKind::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.emit_pat_bind(first, val, num_kind)?;
                }
                Ok(())
            }
            PatKind::Literal(_) | PatKind::NegLiteral(_) | PatKind::Range { .. } => Ok(()),
        }
    }

    fn emit_slice_bind(&mut self, elems: &[Pat], val: Reg) -> R<()> {
        let rest_pos = elems.iter().position(|p| is_rest_pat(p));
        match rest_pos {
            None => {
                for (i, p) in elems.iter().enumerate() {
                    let idx = self.small_int(i as i32)?;
                    let item = self.helper("patSliceElem", &[val, idx])?;
                    self.emit_pat_bind(p, item, NumKind::Unknown)?;
                }
            }
            Some(rp) => {
                for (i, p) in elems[..rp].iter().enumerate() {
                    let idx = self.small_int(i as i32)?;
                    let item = self.helper("patSliceElem", &[val, idx])?;
                    self.emit_pat_bind(p, item, NumKind::Unknown)?;
                }
                let tail_count = elems.len() - rp - 1;
                let total = self.helper("patSliceLen", &[val])?;
                let sub_start = self.small_int(rp as i32)?;
                let tc = self.small_int(tail_count as i32)?;
                let sub_end = self.alloc()?;
                self.emit(Instr::Sub {
                    dst: sub_end,
                    a: total,
                    b: tc,
                })?;
                // Bind `rest @ ..` if present
                if let PatKind::Ident { name, .. } = &elems[rp].kind {
                    let sub = self.helper("patSliceSub", &[val, sub_start, sub_end])?;
                    self.bind_local(name, sub, NumKind::Unknown)?;
                }
                for (j, p) in elems[rp + 1..].iter().enumerate() {
                    let off = self.small_int(j as i32)?;
                    let idx = self.alloc()?;
                    self.emit(Instr::Add {
                        dst: idx,
                        a: sub_end,
                        b: off,
                    })?;
                    let item = self.helper("patSliceElem", &[val, idx])?;
                    self.emit_pat_bind(p, item, NumKind::Unknown)?;
                }
            }
        }
        Ok(())
    }

    /// Refutable pattern match & bind: emits tests that jump to any entry appended to `fails`
    /// if the pattern does NOT match; falls through with all pattern variables bound if it matches!
    pub(super) fn emit_pat_match(
        &mut self,
        pat: &Pat,
        val: Reg,
        fails: &mut Vec<usize>,
    ) -> R<()> {
        match &pat.kind {
            PatKind::Wildcard | PatKind::Rest => Ok(()),
            PatKind::Ident { name, subpat, .. } => {
                // Bind first (or test subpat first, then bind)
                if let Some(sub) = subpat {
                    self.emit_pat_match(sub, val, fails)?;
                }
                self.bind_local(name, val, NumKind::Unknown)?;
                Ok(())
            }
            PatKind::Ref { inner, .. } => {
                let d = self.helper("deref", &[val])?;
                self.emit_pat_match(inner, d, fails)
            }
            PatKind::Literal(lit) => {
                let expected = self.emit_literal(lit)?;
                let ok = self.helper("eq", &[val, expected])?;
                fails.push(self.jump_if_false(ok)?);
                Ok(())
            }
            PatKind::NegLiteral(lit) => {
                let pos = self.emit_literal(lit)?;
                let expected = self.helper("neg", &[pos])?;
                let ok = self.helper("eq", &[val, expected])?;
                fails.push(self.jump_if_false(ok)?);
                Ok(())
            }
            PatKind::Range {
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
                let ok = self.helper("patRangeMatch", &[val, sr, er, inc])?;
                fails.push(self.jump_if_false(ok)?);
                Ok(())
            }
            PatKind::Path(path) => {
                let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
                let resolved = self.resolve_segments(&raw);
                let last = resolved.last().map(|s| s.as_str()).unwrap_or("");
                // Prelude unit enum variants: `None`
                if last == "None" && (resolved.len() == 1 || resolved.get(resolved.len() - 2).map(|s| s.as_str()) == Some("Option")) {
                    let en = self.string("Option")?;
                    let vn = self.string("None")?;
                    let ok = self.helper("patEnumMatch", &[val, en, vn])?;
                    fails.push(self.jump_if_false(ok)?);
                    return Ok(());
                }
                // Known enum unit variant (`Enum::Variant` or glob-imported `Variant`)
                if let Some((ename, vname, _)) = self.syms.lookup_enum_variant(&self.mod_prefix, &raw) {
                    let en = self.string(&ename)?;
                    let vn = self.string(&vname)?;
                    let ok = self.helper("patEnumMatch", &[val, en, vn])?;
                    fails.push(self.jump_if_false(ok)?);
                    return Ok(());
                }
                // Known constant (`const MAX: i32 = ...;`)
                if let Some((_, slot)) = self.syms.lookup_static_global(&self.mod_prefix, &raw) {
                    let expected = self.alloc()?;
                    self.emit(Instr::LoadGlobal {
                        dst: expected,
                        idx: slot,
                    })?;
                    let ok = self.helper("eq", &[val, expected])?;
                    fails.push(self.jump_if_false(ok)?);
                    return Ok(());
                }
                // Multi-segment enum/const path (e.g. `Ordering::Less`, `ErrorKind::NotFound`)
                if resolved.len() >= 2 {
                    let en = self.string(&resolved[resolved.len() - 2])?;
                    let vn = self.string(last)?;
                    let ok = self.helper("patEnumMatch", &[val, en, vn])?;
                    fails.push(self.jump_if_false(ok)?);
                    return Ok(());
                }
                // Single uppercase ident that isn't a known enum variant or const: treat as unit struct or catch-all binding!
                if self.syms.structs.contains_key(last) {
                    let sn = self.string(last)?;
                    let ok = self.helper("patStructMatch", &[val, sn])?;
                    fails.push(self.jump_if_false(ok)?);
                    return Ok(());
                }
                // Otherwise bind as variable (e.g. `match x { T => ... }`)
                self.bind_local(last, val, NumKind::Unknown)?;
                Ok(())
            }
            PatKind::TupleStruct { path, elems } => {
                let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
                let resolved = self.resolve_segments(&raw);
                let last = resolved.last().map(|s| s.as_str()).unwrap_or("");
                let is_struct = resolved.len() == 1 && self.syms.structs.contains_key(last);
                if is_struct {
                    let sn = self.string(last)?;
                    let ok = self.helper("patStructMatch", &[val, sn])?;
                    fails.push(self.jump_if_false(ok)?);
                    for (i, p) in elems.iter().enumerate() {
                        if matches!(p.kind, PatKind::Rest) {
                            break;
                        }
                        let idx = self.small_int(i as i32)?;
                        let fval = self.helper("getTupleIdx", &[val, idx])?;
                        self.emit_pat_match(p, fval, fails)?;
                    }
                } else {
                    // Enum tuple variant (`Some(x)`, `Ok(v)`, `Err(e)`, `Color::Rgb(r, g, b)`)
                    let enum_name = match last {
                        "Some" => "Option",
                        "Ok" | "Err" => "Result",
                        _ if resolved.len() >= 2 => &resolved[resolved.len() - 2],
                        _ => "",
                    };
                    let en = self.string(enum_name)?;
                    let vn = self.string(last)?;
                    let ok = self.helper("patEnumMatch", &[val, en, vn])?;
                    fails.push(self.jump_if_false(ok)?);
                    for (i, p) in elems.iter().enumerate() {
                        if matches!(p.kind, PatKind::Rest) {
                            break;
                        }
                        let idx = self.small_int(i as i32)?;
                        let fval = self.helper("patEnumField", &[val, idx])?;
                        self.emit_pat_match(p, fval, fails)?;
                    }
                }
                Ok(())
            }
            PatKind::Struct { path, fields, .. } => {
                let raw: Vec<String> = path.segments.iter().map(|s| s.ident.clone()).collect();
                let resolved = self.resolve_segments(&raw);
                let last = resolved.last().map(|s| s.as_str()).unwrap_or("");
                let is_enum_var = self.syms.lookup_enum_variant(&self.mod_prefix, &raw).is_some()
                    || (resolved.len() >= 2 && self.syms.enums.contains_key(&resolved[resolved.len() - 2]));
                if is_enum_var {
                    let ename = if resolved.len() >= 2 {
                        resolved[resolved.len() - 2].as_str()
                    } else {
                        ""
                    };
                    let en = self.string(ename)?;
                    let vn = self.string(last)?;
                    let ok = self.helper("patEnumMatch", &[val, en, vn])?;
                    fails.push(self.jump_if_false(ok)?);
                    for f in fields {
                        let fn_r = self.string(&f.name)?;
                        let fval = self.helper("patEnumNamed", &[val, fn_r])?;
                        self.emit_pat_match(&f.pat, fval, fails)?;
                    }
                } else {
                    let sn = self.string(last)?;
                    let ok = self.helper("patStructMatch", &[val, sn])?;
                    fails.push(self.jump_if_false(ok)?);
                    for f in fields {
                        let fn_r = self.string(&f.name)?;
                        let fval = self.helper("getField", &[val, fn_r])?;
                        self.emit_pat_match(&f.pat, fval, fails)?;
                    }
                }
                Ok(())
            }
            PatKind::Tuple(elems) => {
                let rest_pos = elems.iter().position(|p| matches!(p.kind, PatKind::Rest));
                match rest_pos {
                    None => {
                        for (i, p) in elems.iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("getTupleIdx", &[val, idx])?;
                            self.emit_pat_match(p, item, fails)?;
                        }
                    }
                    Some(rp) => {
                        for (i, p) in elems[..rp].iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("getTupleIdx", &[val, idx])?;
                            self.emit_pat_match(p, item, fails)?;
                        }
                        let tail_count = elems.len() - rp - 1;
                        if tail_count > 0 {
                            let total = self.helper("patSliceLen", &[val])?;
                            for (j, p) in elems[rp + 1..].iter().enumerate() {
                                let off = self.small_int((tail_count - j) as i32)?;
                                let idx = self.alloc()?;
                                self.emit(Instr::Sub {
                                    dst: idx,
                                    a: total,
                                    b: off,
                                })?;
                                let item = self.helper("getTupleIdx", &[val, idx])?;
                                self.emit_pat_match(p, item, fails)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            PatKind::Slice(elems) => {
                let len = self.helper("patSliceLen", &[val])?;
                let rest_pos = elems.iter().position(|p| is_rest_pat(p));
                match rest_pos {
                    None => {
                        let expected = self.small_int(elems.len() as i32)?;
                        let ok = self.alloc()?;
                        self.emit(Instr::Eq {
                            dst: ok,
                            a: len,
                            b: expected,
                        })?;
                        fails.push(self.jump_if_false(ok)?);
                        for (i, p) in elems.iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("patSliceElem", &[val, idx])?;
                            self.emit_pat_match(p, item, fails)?;
                        }
                    }
                    Some(rp) => {
                        let min_len = elems.len() - 1;
                        let expected = self.small_int(min_len as i32)?;
                        let ok = self.alloc()?;
                        self.emit(Instr::Ge {
                            dst: ok,
                            a: len,
                            b: expected,
                        })?;
                        fails.push(self.jump_if_false(ok)?);
                        for (i, p) in elems[..rp].iter().enumerate() {
                            let idx = self.small_int(i as i32)?;
                            let item = self.helper("patSliceElem", &[val, idx])?;
                            self.emit_pat_match(p, item, fails)?;
                        }
                        let tail_count = elems.len() - rp - 1;
                        let sub_start = self.small_int(rp as i32)?;
                        let tc = self.small_int(tail_count as i32)?;
                        let sub_end = self.alloc()?;
                        self.emit(Instr::Sub {
                            dst: sub_end,
                            a: len,
                            b: tc,
                        })?;
                        if let PatKind::Ident { name, .. } = &elems[rp].kind {
                            let sub = self.helper("patSliceSub", &[val, sub_start, sub_end])?;
                            self.bind_local(name, sub, NumKind::Unknown)?;
                        }
                        for (j, p) in elems[rp + 1..].iter().enumerate() {
                            let off = self.small_int(j as i32)?;
                            let idx = self.alloc()?;
                            self.emit(Instr::Add {
                                dst: idx,
                                a: sub_end,
                                b: off,
                            })?;
                            let item = self.helper("patSliceElem", &[val, idx])?;
                            self.emit_pat_match(p, item, fails)?;
                        }
                    }
                }
                Ok(())
            }
            PatKind::Or(alts) => {
                // Pre-allocate shared registers for bindings common across OR-alternatives
                let mut shared_names = std::collections::HashSet::new();
                if let Some(first) = alts.first() {
                    super::scope::collect_pat_names(first, &mut shared_names);
                }
                let mut shared_slots: Vec<(String, Reg)> = Vec::new();
                for name in &shared_names {
                    let u = self.undefined()?;
                    let slot = self.bind_local(name, u, NumKind::Unknown)?;
                    shared_slots.push((name.clone(), slot));
                }

                let mut success_jumps = Vec::new();
                for (idx, alt) in alts.iter().enumerate() {
                    let is_last = idx + 1 == alts.len();
                    self.push_scope();
                    let mut alt_fails = Vec::new();
                    self.emit_pat_match(alt, val, &mut alt_fails)?;
                    // Copy alt's bound vars into shared slots
                    for (name, _) in &shared_slots {
                        if let Some(alt_lv) = self.scopes.last().unwrap().get(name).copied() {
                            let v = self.load_local(alt_lv)?;
                            if let Some(outer_lv) = self.scopes[self.scopes.len() - 2].get(name).copied() {
                                self.store_local(outer_lv, v)?;
                            }
                        }
                    }
                    self.pop_scope();
                    if !is_last {
                        success_jumps.push(self.jump()?);
                        let next_pc = self.here();
                        for fj in alt_fails {
                            self.patch(fj, next_pc)?;
                        }
                    } else {
                        fails.extend(alt_fails);
                    }
                }
                let matched_pc = self.here();
                for sj in success_jumps {
                    self.patch(sj, matched_pc)?;
                }
                Ok(())
            }
        }
    }
}

fn is_rest_pat(p: &Pat) -> bool {
    match &p.kind {
        PatKind::Rest => true,
        PatKind::Ident {
            subpat: Some(sub), ..
        } => matches!(sub.kind, PatKind::Rest),
        _ => false,
    }
}
