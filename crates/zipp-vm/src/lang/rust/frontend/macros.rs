//! Built-in Rust macros (`println!`, `format!`, `vec!`, `assert*!`, `panic!`, `dbg!`,
//! `matches!`, `json!`, `write!`, `concat!`, `stringify!`, `include_str!`, `env!`, `cfg!`)
//! plus declarative `macro_rules!` token-tree matching & expansion.

use super::emitter::{RsEmitter, R};
use super::scope::extract_format_captures;
use crate::bytecode::Reg;
use std::collections::HashMap;
use zipp_rsparse::ast::*;
use zipp_rsparse::token::{Token, TokenKind};
use zipp_rsparse::{parse_expr_tokens, Limits};

impl<'a> RsEmitter<'a> {
    pub(super) fn emit_macro_call(&mut self, mc: &MacroCall) -> R<Reg> {
        let name = mc.path.last_ident();
        // Check user-defined `macro_rules!` first
        if let Some(rules) = self.syms.macro_rules.get(name).cloned() {
            if let Some(expanded) = expand_macro_rules(&rules, &mc.tokens) {
                let expr = parse_expr_tokens(self.file.source, expanded, Limits::DEFAULT)
                    .map_err(|e| format!("Rust macro `{name}!` expansion error: {e}"))?;
                return self.emit_expr(&expr);
            }
        }

        match name {
            "println" => self.emit_format_call("println", &mc.tokens, true),
            "print" => self.emit_format_call("print", &mc.tokens, false),
            "eprintln" => self.emit_format_call("eprintln", &mc.tokens, true),
            "eprint" => self.emit_format_call("eprint", &mc.tokens, false),
            "format" | "format_args" => self.emit_format_call("format", &mc.tokens, false),
            "write" => self.emit_write_macro(&mc.tokens, false),
            "writeln" => self.emit_write_macro(&mc.tokens, true),
            "panic" => {
                if mc.tokens.is_empty() {
                    let msg = self.string("explicit panic")?;
                    self.helper("panic", &[msg])
                } else {
                    let formatted = self.emit_format_call("format", &mc.tokens, false)?;
                    self.helper("panic", &[formatted])
                }
            }
            "todo" => {
                let msg = if mc.tokens.is_empty() {
                    self.string("not yet implemented")?
                } else {
                    self.emit_format_call("format", &mc.tokens, false)?
                };
                self.helper("panic", &[msg])
            }
            "unimplemented" => {
                let msg = if mc.tokens.is_empty() {
                    self.string("not implemented")?
                } else {
                    self.emit_format_call("format", &mc.tokens, false)?
                };
                self.helper("panic", &[msg])
            }
            "unreachable" => {
                let msg = if mc.tokens.is_empty() {
                    self.string("entered unreachable code")?
                } else {
                    self.emit_format_call("format", &mc.tokens, false)?
                };
                self.helper("panic", &[msg])
            }
            "vec" => self.emit_vec_macro(&mc.tokens),
            "assert" | "debug_assert" => self.emit_assert_macro(&mc.tokens),
            "assert_eq" | "debug_assert_eq" => self.emit_assert_cmp_macro("assertEq", &mc.tokens),
            "assert_ne" | "debug_assert_ne" => self.emit_assert_cmp_macro("assertNe", &mc.tokens),
            "dbg" => self.emit_dbg_macro(&mc.tokens),
            "matches" => self.emit_matches_macro(&mc.tokens),
            "json" => self.emit_json_macro(&mc.tokens),
            "concat" => self.emit_concat_macro(&mc.tokens),
            "stringify" => {
                let text = tokens_to_source(self.file.source, &mc.tokens);
                self.string(&text)
            }
            "file" => self.string(self.file.file),
            "line" => {
                let line = self.file.line_of(mc.span.start);
                self.small_int(line as i32)
            }
            "column" => self.small_int(1),
            "module_path" => {
                let mp = if self.mod_prefix.is_empty() {
                    "crate".to_owned()
                } else {
                    format!("crate::{}", self.mod_prefix)
                };
                self.string(&mp)
            }
            "cfg" => {
                let s = tokens_to_source(self.file.source, &mc.tokens);
                let ok = match s.trim() {
                    "test" => self.file.is_test_mode,
                    "debug_assertions" => false,
                    "unix" => cfg!(unix),
                    "windows" => cfg!(windows),
                    _ => true,
                };
                self.boolean(ok)
            }
            "env" | "option_env" => {
                let args = split_top_level_commas(&mc.tokens);
                let key = if let Some(first) = args.first() {
                    let e = parse_expr_tokens(self.file.source, first.clone(), Limits::DEFAULT)
                        .map_err(|e| format!("env! arg error: {e}"))?;
                    self.emit_expr(&e)?
                } else {
                    return Err("env! requires a key argument".into());
                };
                let p1 = self.string("std")?;
                let p2 = self.string("env")?;
                let p3 = self.string(if name == "env" { "var" } else { "var_os" })?;
                let path_arr = self.js_array(&[p1, p2, p3])?;
                let arg_arr = self.js_array(&[key])?;
                let res = self.helper("callAssoc", &[path_arr, arg_arr])?;
                if name == "env" {
                    let m = self.string("unwrap")?;
                    let empty = self.js_array(&[])?;
                    self.helper("callMethod", &[res, m, empty])
                } else {
                    Ok(res)
                }
            }
            "include_str" => {
                let args = split_top_level_commas(&mc.tokens);
                let rel = match args.first().and_then(|ts| ts.first()).map(|t| &t.kind) {
                    Some(TokenKind::Str(s)) => s.clone(),
                    _ => return Err("include_str! expects a string literal path".into()),
                };
                let content = (self.file.read_relative)(&rel)?;
                self.string(&content)
            }
            "include_bytes" => {
                let args = split_top_level_commas(&mc.tokens);
                let rel = match args.first().and_then(|ts| ts.first()).map(|t| &t.kind) {
                    Some(TokenKind::Str(s)) => s.clone(),
                    _ => return Err("include_bytes! expects a string literal path".into()),
                };
                let content = (self.file.read_relative)(&rel)?;
                let mut regs = Vec::with_capacity(content.len());
                for &b in content.as_bytes() {
                    regs.push(self.small_int(b as i32)?);
                }
                let arr = self.js_array(&regs)?;
                self.helper("mkVec", &[arr])
            }
            "info" | "warn" | "error" | "debug" | "trace" => {
                let level = name.to_ascii_uppercase();
                self.emit_log_macro(&level, &mc.tokens)
            }
            "anyhow" => self.emit_format_call("format", &mc.tokens, false),
            "bail" => {
                let msg = self.emit_format_call("format", &mc.tokens, false)?;
                let err = self.helper("Err", &[msg])?;
                let ret_err = if let Some(ref et) = self.ret_err_type.clone() {
                    let et_r = self.string(et)?;
                    self.helper("tryErr", &[err, et_r])?
                } else {
                    err
                };
                self.emit(crate::bytecode::Instr::Return { src: ret_err })?;
                self.undefined()
            }
            "ensure" => {
                let chunks = split_top_level_commas(&mc.tokens);
                if chunks.is_empty() {
                    return Err("ensure! requires a condition".into());
                }
                let cond_e = parse_expr_tokens(self.file.source, chunks[0].clone(), Limits::DEFAULT)
                    .map_err(|e| format!("ensure! cond error: {e}"))?;
                let cond_r = self.emit_expr(&cond_e)?;
                let j_ok = self.jump_if_true(cond_r)?;
                let msg = if chunks.len() > 1 {
                    let rest = join_chunks_with_commas(&chunks[1..]);
                    self.emit_format_call("format", &rest, false)?
                } else {
                    self.string("condition failed")?
                };
                let err = self.helper("Err", &[msg])?;
                self.emit(crate::bytecode::Instr::Return { src: err })?;
                self.patch(j_ok, self.here())?;
                self.undefined()
            }
            "join" => {
                // `tokio::join!(a, b)` -> `(a.await, b.await)`
                let parts = split_top_level_commas(&mc.tokens);
                let mut regs = Vec::with_capacity(parts.len());
                for p in parts {
                    if p.is_empty() {
                        continue;
                    }
                    let e = parse_expr_tokens(self.file.source, p, Limits::DEFAULT)
                        .map_err(|e| format!("join! arg error: {e}"))?;
                    let r = self.emit_expr(&e)?;
                    if self.proto.is_async {
                        let dst = self.alloc()?;
                        self.emit(crate::bytecode::Instr::Await { dst, val: r })?;
                        regs.push(dst);
                    } else {
                        regs.push(r);
                    }
                }
                let arr = self.js_array(&regs)?;
                self.helper("mkTuple", &[arr])
            }
            _ => Err(format!("Rust: unsupported macro `{name}!`")),
        }
    }

    fn emit_format_call(
        &mut self,
        helper_name: &str,
        tokens: &[Token],
        allow_empty: bool,
    ) -> R<Reg> {
        if tokens.is_empty() {
            if allow_empty {
                return self.helper(helper_name, &[]);
            }
            let empty = self.string("")?;
            let pos = self.js_array(&[])?;
            return self.helper(helper_name, &[empty, pos]);
        }
        let arg_chunks = split_top_level_commas(tokens);
        if arg_chunks.is_empty() {
            return self.helper(helper_name, &[]);
        }
        let tmpl_expr = parse_expr_tokens(self.file.source, arg_chunks[0].clone(), Limits::DEFAULT)
            .map_err(|e| format!("{helper_name}! template parse error: {e}"))?;
        let tmpl_lit = match &tmpl_expr.kind {
            ExprKind::Literal(Literal::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let tmpl_reg = self.emit_expr(&tmpl_expr)?;
        let mut pos_regs = Vec::new();
        let mut named: Vec<(String, Reg)> = Vec::new();

        for chunk in arg_chunks.iter().skip(1) {
            if chunk.is_empty() {
                continue;
            }
            // Named format argument: `name = expr`
            if chunk.len() >= 3
                && matches!(chunk[0].kind, TokenKind::Ident(_))
                && matches!(chunk[1].kind, TokenKind::Eq)
            {
                let TokenKind::Ident(k) = &chunk[0].kind else {
                    unreachable!()
                };
                let val_expr =
                    parse_expr_tokens(self.file.source, chunk[2..].to_vec(), Limits::DEFAULT)
                        .map_err(|e| format!("{helper_name}! named arg `{k}` error: {e}"))?;
                let vreg = self.emit_expr(&val_expr)?;
                named.push((k.clone(), vreg));
            } else {
                let val_expr = parse_expr_tokens(self.file.source, chunk.clone(), Limits::DEFAULT)
                    .map_err(|e| format!("{helper_name}! arg error: {e}"))?;
                pos_regs.push(self.emit_expr(&val_expr)?);
            }
        }

        // Rust 1.58+ inline captured format arguments: `println!("x={x}, y={y:?}")`
        if let Some(ref s) = tmpl_lit {
            for cap in extract_format_captures(s) {
                if !named.iter().any(|(k, _)| k == &cap) {
                    let vreg = self.emit_path(&Path::simple(cap.clone(), tmpl_expr.span))?;
                    named.push((cap, vreg));
                }
            }
        }

        let pos_arr = self.js_array(&pos_regs)?;
        if named.is_empty() {
            self.helper(helper_name, &[tmpl_reg, pos_arr])
        } else {
            let obj = self.new_js_obj()?;
            for (k, v) in named {
                self.set_prop(obj, &k, v)?;
            }
            self.helper(helper_name, &[tmpl_reg, pos_arr, obj])
        }
    }

    fn emit_log_macro(&mut self, level: &str, tokens: &[Token]) -> R<Reg> {
        let lvl = self.string(level)?;
        let formatted = self.emit_format_call("format", tokens, true)?;
        let tmpl = self.string("{}")?;
        let pos = self.js_array(&[formatted])?;
        self.helper("logMsg", &[lvl, tmpl, pos])
    }

    fn emit_write_macro(&mut self, tokens: &[Token], newline: bool) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        if chunks.is_empty() {
            return Err("write! requires a destination argument".into());
        }
        let dst_expr = parse_expr_tokens(self.file.source, chunks[0].clone(), Limits::DEFAULT)
            .map_err(|e| format!("write! dst error: {e}"))?;
        let dst_reg = self.emit_expr(&dst_expr)?;
        if chunks.len() == 1 {
            let empty = self.string("")?;
            let pos = self.js_array(&[])?;
            let null_r = self.null()?;
            let nl = self.boolean(newline)?;
            return self.helper("writeFmt", &[dst_reg, empty, pos, null_r, nl]);
        }
        // Reassemble remaining chunks into format args
        let mut rest_tokens = Vec::new();
        for (i, ch) in chunks.iter().skip(1).enumerate() {
            if i > 0 {
                rest_tokens.push(Token::new(TokenKind::Comma, 0, 0));
            }
            rest_tokens.extend(ch.iter().cloned());
        }
        let formatted = self.emit_format_call("format", &rest_tokens, true)?;
        let tmpl = self.string("{}")?;
        let pos = self.js_array(&[formatted])?;
        let null_r = self.null()?;
        let nl = self.boolean(newline)?;
        self.helper("writeFmt", &[dst_reg, tmpl, pos, null_r, nl])
    }

    fn emit_vec_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        if tokens.is_empty() {
            let arr = self.js_array(&[])?;
            return self.helper("mkVec", &[arr]);
        }
        // Check `vec![elem; count]`
        if let Some(semi_pos) = find_top_level_token(tokens, &TokenKind::Semi) {
            let elem_e = parse_expr_tokens(
                self.file.source,
                tokens[..semi_pos].to_vec(),
                Limits::DEFAULT,
            )
            .map_err(|e| format!("vec! repeat elem error: {e}"))?;
            let count_e = parse_expr_tokens(
                self.file.source,
                tokens[semi_pos + 1..].to_vec(),
                Limits::DEFAULT,
            )
            .map_err(|e| format!("vec! repeat count error: {e}"))?;
            let er = self.emit_expr(&elem_e)?;
            let cr = self.emit_expr(&count_e)?;
            return self.helper("mkRepeat", &[er, cr]);
        }
        let chunks = split_top_level_commas(tokens);
        let mut regs = Vec::with_capacity(chunks.len());
        for ch in chunks {
            if ch.is_empty() {
                continue;
            }
            let e = parse_expr_tokens(self.file.source, ch, Limits::DEFAULT)
                .map_err(|e| format!("vec! elem error: {e}"))?;
            regs.push(self.emit_expr(&e)?);
        }
        let arr = self.js_array(&regs)?;
        self.helper("mkVec", &[arr])
    }

    fn emit_assert_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        if chunks.is_empty() {
            return Err("assert! requires a condition".into());
        }
        let cond_e = parse_expr_tokens(self.file.source, chunks[0].clone(), Limits::DEFAULT)
            .map_err(|e| format!("assert! cond error: {e}"))?;
        let cond_r = self.emit_expr(&cond_e)?;
        if chunks.len() > 1 {
            let msg_tokens = join_chunks_with_commas(&chunks[1..]);
            let msg_r = self.emit_format_call("format", &msg_tokens, false)?;
            self.helper("assert", &[cond_r, msg_r])
        } else {
            self.helper("assert", &[cond_r])
        }
    }

    fn emit_assert_cmp_macro(&mut self, helper_name: &str, tokens: &[Token]) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        if chunks.len() < 2 {
            return Err(format!("{helper_name}: requires left and right arguments"));
        }
        let lhs_e = parse_expr_tokens(self.file.source, chunks[0].clone(), Limits::DEFAULT)
            .map_err(|e| format!("assert lhs error: {e}"))?;
        let rhs_e = parse_expr_tokens(self.file.source, chunks[1].clone(), Limits::DEFAULT)
            .map_err(|e| format!("assert rhs error: {e}"))?;
        let lhs_r = self.emit_expr(&lhs_e)?;
        let rhs_r = self.emit_expr(&rhs_e)?;
        if chunks.len() > 2 {
            let msg_tokens = join_chunks_with_commas(&chunks[2..]);
            let msg_r = self.emit_format_call("format", &msg_tokens, false)?;
            self.helper(helper_name, &[lhs_r, rhs_r, msg_r])
        } else {
            self.helper(helper_name, &[lhs_r, rhs_r])
        }
    }

    fn emit_dbg_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        if chunks.is_empty() {
            return self.undefined();
        }
        let mut regs = Vec::with_capacity(chunks.len());
        for ch in chunks {
            if ch.is_empty() {
                continue;
            }
            let text = tokens_to_source(self.file.source, &ch);
            let e = parse_expr_tokens(self.file.source, ch, Limits::DEFAULT)
                .map_err(|e| format!("dbg! error: {e}"))?;
            let val = self.emit_expr(&e)?;
            let label = self.string(&text)?;
            regs.push(self.helper("dbg", &[label, val])?);
        }
        if regs.len() == 1 {
            Ok(regs[0])
        } else {
            let arr = self.js_array(&regs)?;
            self.helper("mkTuple", &[arr])
        }
    }

    fn emit_matches_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        if chunks.len() < 2 {
            return Err("matches! requires (expr, pat)".into());
        }
        let expr_str = tokens_to_source(self.file.source, &chunks[0]);
        let pat_str = tokens_to_source(self.file.source, &chunks[1]);
        let synth = format!("match ({expr_str}) {{ {pat_str} => true, _ => false }}");
        let parsed = zipp_rsparse::parse(&format!("fn _m() {{ {synth} }}"), Limits::DEFAULT)
            .map_err(|e| format!("matches! parse error: {e}"))?;
        if let Some(Item {
            kind: ItemKind::Fn(f),
            ..
        }) = parsed.items.first()
        {
            if let Some(tail) = f.body.as_ref().and_then(|b| b.tail.as_deref()) {
                return self.emit_expr(tail);
            }
        }
        Err("matches! expansion failed".into())
    }

    /// `serde_json::json!({ "key": expr, "arr": [1, 2, null, true] })`
    fn emit_json_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        let raw = self.emit_json_tokens(tokens)?;
        self.helper("mkJson", &[raw])
    }

    fn emit_json_tokens(&mut self, tokens: &[Token]) -> R<Reg> {
        if tokens.is_empty() {
            return self.null();
        }
        if tokens.len() == 1 {
            match &tokens[0].kind {
                TokenKind::Ident(s) if s == "null" => return self.null(),
                TokenKind::True => return self.boolean(true),
                TokenKind::False => return self.boolean(false),
                TokenKind::Str(s) => return self.string(s),
                _ => {}
            }
        }
        // JSON object `{ "k": v, ... }`
        if matches!(tokens.first().map(|t| &t.kind), Some(TokenKind::LBrace))
            && matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::RBrace))
        {
            let inner = &tokens[1..tokens.len() - 1];
            let entries = split_top_level_commas(inner);
            let obj = self.new_js_obj()?;
            for entry in entries {
                if entry.is_empty() {
                    continue;
                }
                let Some(colon) = find_top_level_token(&entry, &TokenKind::Colon) else {
                    return Err("json! object entry missing `:`".into());
                };
                let key = match &entry[0].kind {
                    TokenKind::Str(s) => s.clone(),
                    TokenKind::Ident(s) => s.clone(),
                    _ => return Err("json! object key must be string or ident".into()),
                };
                let val_r = self.emit_json_tokens(&entry[colon + 1..])?;
                self.set_prop(obj, &key, val_r)?;
            }
            return Ok(obj);
        }
        // JSON array `[a, b, c]`
        if matches!(tokens.first().map(|t| &t.kind), Some(TokenKind::LBracket))
            && matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::RBracket))
        {
            let inner = &tokens[1..tokens.len() - 1];
            let elems = split_top_level_commas(inner);
            let mut regs = Vec::with_capacity(elems.len());
            for el in elems {
                if el.is_empty() {
                    continue;
                }
                regs.push(self.emit_json_tokens(&el)?);
            }
            return self.js_array(&regs);
        }
        // General Rust expression inside `json!`: evaluate and convert via `toJsonRaw`
        let expr = parse_expr_tokens(self.file.source, tokens.to_vec(), Limits::DEFAULT)
            .map_err(|e| format!("json! expr error: {e}"))?;
        let val = self.emit_expr(&expr)?;
        let rt_obj = self.prop(self.r_rt, "__rt")?;
        let callee = self.prop(rt_obj, "toJsonRaw")?;
        let dst = self.alloc()?;
        self.emit(crate::bytecode::Instr::Call {
            dst,
            callee,
            arg_base: val,
            argc: 1,
        })?;
        Ok(dst)
    }

    fn emit_concat_macro(&mut self, tokens: &[Token]) -> R<Reg> {
        let chunks = split_top_level_commas(tokens);
        let mut out = String::new();
        for ch in chunks {
            if let Some(tok) = ch.first() {
                match &tok.kind {
                    TokenKind::Str(s) => out.push_str(s),
                    TokenKind::Char(c) => out.push(*c),
                    TokenKind::Int(v, _) => out.push_str(&v.to_string()),
                    TokenKind::Float(v, _) => out.push_str(&v.to_string()),
                    TokenKind::True => out.push_str("true"),
                    TokenKind::False => out.push_str("false"),
                    _ => {}
                }
            }
        }
        self.string(&out)
    }
}

// ---- Token-tree helpers ----

pub(super) fn split_top_level_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut parts = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0usize;
    for tok in tokens {
        match tok.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                depth += 1;
                cur.push(tok.clone());
            }
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                cur.push(tok.clone());
            }
            TokenKind::Comma if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(tok.clone()),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn find_top_level_token(tokens: &[Token], needle: &TokenKind) -> Option<usize> {
    let mut depth = 0usize;
    for (i, tok) in tokens.iter().enumerate() {
        if depth == 0 && &tok.kind == needle {
            return Some(i);
        }
        match tok.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn join_chunks_with_commas(chunks: &[Vec<Token>]) -> Vec<Token> {
    let mut out = Vec::new();
    for (i, ch) in chunks.iter().enumerate() {
        if i > 0 {
            out.push(Token::new(TokenKind::Comma, 0, 0));
        }
        out.extend(ch.iter().cloned());
    }
    out
}

fn tokens_to_source(_source: &str, tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i > 0 {
            let needs_space = !matches!(
                tok.kind,
                TokenKind::Comma
                    | TokenKind::Semi
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
                    | TokenKind::Dot
                    | TokenKind::ColonColon
                    | TokenKind::LParen
                    | TokenKind::LBracket
                    | TokenKind::Question
            ) && !matches!(
                tokens[i - 1].kind,
                TokenKind::LParen
                    | TokenKind::LBracket
                    | TokenKind::Dot
                    | TokenKind::ColonColon
                    | TokenKind::Amp
                    | TokenKind::Bang
                    | TokenKind::Minus
            );
            if needs_space {
                out.push(' ');
            }
        }
        match &tok.kind {
            TokenKind::Ident(s) => out.push_str(s),
            TokenKind::Lifetime(s) => {
                out.push('\'');
                out.push_str(s);
            }
            TokenKind::Int(v, _) => out.push_str(&v.to_string()),
            TokenKind::Float(v, _) => out.push_str(&v.to_string()),
            TokenKind::Char(c) => {
                out.push('\'');
                out.push_str(&c.escape_default().to_string());
                out.push('\'');
            }
            TokenKind::Byte(b) => out.push_str(&format!("b'\\x{b:02x}'")),
            TokenKind::Str(s) => out.push_str(&format!("{s:?}")),
            TokenKind::ByteStr(bs) => out.push_str(&format!("b{:?}", String::from_utf8_lossy(bs))),
            TokenKind::As => out.push_str("as"),
            TokenKind::Async => out.push_str("async"),
            TokenKind::Await => out.push_str("await"),
            TokenKind::Break => out.push_str("break"),
            TokenKind::Const => out.push_str("const"),
            TokenKind::Continue => out.push_str("continue"),
            TokenKind::Crate => out.push_str("crate"),
            TokenKind::Dyn => out.push_str("dyn"),
            TokenKind::Else => out.push_str("else"),
            TokenKind::Enum => out.push_str("enum"),
            TokenKind::Extern => out.push_str("extern"),
            TokenKind::False => out.push_str("false"),
            TokenKind::Fn => out.push_str("fn"),
            TokenKind::For => out.push_str("for"),
            TokenKind::If => out.push_str("if"),
            TokenKind::Impl => out.push_str("impl"),
            TokenKind::In => out.push_str("in"),
            TokenKind::Let => out.push_str("let"),
            TokenKind::Loop => out.push_str("loop"),
            TokenKind::Match => out.push_str("match"),
            TokenKind::Mod => out.push_str("mod"),
            TokenKind::Move => out.push_str("move"),
            TokenKind::Mut => out.push_str("mut"),
            TokenKind::Pub => out.push_str("pub"),
            TokenKind::Ref => out.push_str("ref"),
            TokenKind::Return => out.push_str("return"),
            TokenKind::SelfLower => out.push_str("self"),
            TokenKind::SelfUpper => out.push_str("Self"),
            TokenKind::Static => out.push_str("static"),
            TokenKind::Struct => out.push_str("struct"),
            TokenKind::Super => out.push_str("super"),
            TokenKind::Trait => out.push_str("trait"),
            TokenKind::True => out.push_str("true"),
            TokenKind::Type => out.push_str("type"),
            TokenKind::Unsafe => out.push_str("unsafe"),
            TokenKind::Use => out.push_str("use"),
            TokenKind::Where => out.push_str("where"),
            TokenKind::While => out.push_str("while"),
            TokenKind::LParen => out.push('('),
            TokenKind::RParen => out.push(')'),
            TokenKind::LBracket => out.push('['),
            TokenKind::RBracket => out.push(']'),
            TokenKind::LBrace => out.push('{'),
            TokenKind::RBrace => out.push('}'),
            TokenKind::Comma => out.push(','),
            TokenKind::Semi => out.push(';'),
            TokenKind::Colon => out.push(':'),
            TokenKind::ColonColon => out.push_str("::"),
            TokenKind::Dot => out.push('.'),
            TokenKind::DotDot => out.push_str(".."),
            TokenKind::DotDotEq => out.push_str("..="),
            TokenKind::DotDotDot => out.push_str("..."),
            TokenKind::Arrow => out.push_str("->"),
            TokenKind::FatArrow => out.push_str("=>"),
            TokenKind::Pound => out.push('#'),
            TokenKind::Dollar => out.push('$'),
            TokenKind::Question => out.push('?'),
            TokenKind::At => out.push('@'),
            TokenKind::Tilde => out.push('~'),
            TokenKind::Plus => out.push('+'),
            TokenKind::Minus => out.push('-'),
            TokenKind::Star => out.push('*'),
            TokenKind::Slash => out.push('/'),
            TokenKind::Percent => out.push('%'),
            TokenKind::Caret => out.push('^'),
            TokenKind::Bang => out.push('!'),
            TokenKind::Amp => out.push('&'),
            TokenKind::Pipe => out.push('|'),
            TokenKind::AmpAmp => out.push_str("&&"),
            TokenKind::PipePipe => out.push_str("||"),
            TokenKind::Shl => out.push_str("<<"),
            TokenKind::Shr => out.push_str(">>"),
            TokenKind::Eq => out.push('='),
            TokenKind::EqEq => out.push_str("=="),
            TokenKind::NotEq => out.push_str("!="),
            TokenKind::Lt => out.push('<'),
            TokenKind::Le => out.push_str("<="),
            TokenKind::Gt => out.push('>'),
            TokenKind::Ge => out.push_str(">="),
            TokenKind::PlusEq => out.push_str("+="),
            TokenKind::MinusEq => out.push_str("-="),
            TokenKind::StarEq => out.push_str("*="),
            TokenKind::SlashEq => out.push_str("/="),
            TokenKind::PercentEq => out.push_str("%="),
            TokenKind::CaretEq => out.push_str("^="),
            TokenKind::AmpEq => out.push_str("&="),
            TokenKind::PipeEq => out.push_str("|="),
            TokenKind::ShlEq => out.push_str("<<="),
            TokenKind::ShrEq => out.push_str(">>="),
            TokenKind::Eof => {}
        }
    }
    out
}

// ---- Declarative `macro_rules!` matcher & expander ----

fn expand_macro_rules(rules: &[MacroRule], input: &[Token]) -> Option<Vec<Token>> {
    for rule in rules {
        let mut bindings: HashMap<String, Vec<Token>> = HashMap::new();
        let mut rep_bindings: HashMap<String, Vec<Vec<Token>>> = HashMap::new();
        if match_rule(&rule.matcher, input, &mut bindings, &mut rep_bindings) {
            return Some(substitute_body(&rule.body, &bindings, &rep_bindings));
        }
    }
    None
}

fn match_rule(
    pat: &[Token],
    input: &[Token],
    bindings: &mut HashMap<String, Vec<Token>>,
    rep_bindings: &mut HashMap<String, Vec<Vec<Token>>>,
) -> bool {
    let mut pi = 0usize;
    let mut ii = 0usize;
    while pi < pat.len() {
        // Repetition `$( ... ) sep *|+`
        if matches!(pat[pi].kind, TokenKind::Dollar)
            && matches!(pat.get(pi + 1).map(|t| &t.kind), Some(TokenKind::LParen))
        {
            let mut depth = 1usize;
            let mut end_p = pi + 2;
            while end_p < pat.len() && depth > 0 {
                match pat[end_p].kind {
                    TokenKind::LParen => depth += 1,
                    TokenKind::RParen => depth -= 1,
                    _ => {}
                }
                end_p += 1;
            }
            let inner = &pat[pi + 2..end_p - 1];
            let mut next_p = end_p;
            let sep = if !matches!(
                pat.get(next_p).map(|t| &t.kind),
                Some(TokenKind::Star | TokenKind::Plus | TokenKind::Question)
            ) {
                let s = pat.get(next_p).map(|t| t.kind.clone());
                next_p += 1;
                s
            } else {
                None
            };
            let op = pat.get(next_p).map(|t| t.kind.clone());
            next_p += 1;
            let mut count = 0usize;
            while ii < input.len() {
                if count > 0 {
                    if let Some(ref s) = sep {
                        if input.get(ii).map(|t| &t.kind) == Some(s) {
                            ii += 1;
                            if ii >= input.len() {
                                break; // trailing separator
                            }
                        } else {
                            break;
                        }
                    }
                }
                let mut sub_b = HashMap::new();
                let mut sub_r = HashMap::new();
                // Capture up to next separator or end
                let seg_end = if let Some(ref s) = sep {
                    find_top_level_token(&input[ii..], s)
                        .map(|off| ii + off)
                        .unwrap_or(input.len())
                } else {
                    (ii + 1).min(input.len())
                };
                if !match_rule(inner, &input[ii..seg_end], &mut sub_b, &mut sub_r) {
                    break;
                }
                for (k, v) in sub_b {
                    rep_bindings.entry(k).or_default().push(v);
                }
                ii = seg_end;
                count += 1;
            }
            if op == Some(TokenKind::Plus) && count == 0 {
                return false;
            }
            pi = next_p;
            continue;
        }

        // Metavariable `$name:frag`
        if matches!(pat[pi].kind, TokenKind::Dollar)
            && matches!(pat.get(pi + 1).map(|t| &t.kind), Some(TokenKind::Ident(_)))
            && matches!(pat.get(pi + 2).map(|t| &t.kind), Some(TokenKind::Colon))
        {
            let TokenKind::Ident(var) = &pat[pi + 1].kind else {
                unreachable!()
            };
            let frag = match pat.get(pi + 3).map(|t| &t.kind) {
                Some(TokenKind::Ident(f)) => f.as_str(),
                _ => "tt",
            };
            pi += 4;
            let stop_tok = pat.get(pi).map(|t| &t.kind);
            if frag == "ident" || frag == "literal" || frag == "tt" {
                let Some(tok) = input.get(ii) else {
                    return false;
                };
                bindings.insert(var.clone(), vec![tok.clone()]);
                ii += 1;
            } else {
                // `expr`, `ty`, `pat`, `stmt`, `path`: consume until `stop_tok` at depth 0
                let start = ii;
                let mut depth = 0usize;
                while ii < input.len() {
                    if depth == 0 && stop_tok == Some(&input[ii].kind) {
                        break;
                    }
                    match input[ii].kind {
                        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                        TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                            depth = depth.saturating_sub(1);
                        }
                        _ => {}
                    }
                    ii += 1;
                }
                if ii == start {
                    return false;
                }
                bindings.insert(var.clone(), input[start..ii].to_vec());
            }
            continue;
        }

        // Literal token match
        if input.get(ii).map(|t| &t.kind) != Some(&pat[pi].kind) {
            return false;
        }
        pi += 1;
        ii += 1;
    }
    ii == input.len()
}

fn substitute_body(
    body: &[Token],
    bindings: &HashMap<String, Vec<Token>>,
    rep_bindings: &HashMap<String, Vec<Vec<Token>>>,
) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        if matches!(body[i].kind, TokenKind::Dollar)
            && matches!(body.get(i + 1).map(|t| &t.kind), Some(TokenKind::LParen))
        {
            let mut depth = 1usize;
            let mut end_p = i + 2;
            while end_p < body.len() && depth > 0 {
                match body[end_p].kind {
                    TokenKind::LParen => depth += 1,
                    TokenKind::RParen => depth -= 1,
                    _ => {}
                }
                end_p += 1;
            }
            let inner = &body[i + 2..end_p - 1];
            let mut next_i = end_p;
            let sep = if !matches!(
                body.get(next_i).map(|t| &t.kind),
                Some(TokenKind::Star | TokenKind::Plus | TokenKind::Question)
            ) {
                let s = body.get(next_i).cloned();
                next_i += 1;
                s
            } else {
                None
            };
            next_i += 1; // skip `*` / `+`
            // Determine repeat count from first repeated variable referenced inside `inner`
            let mut n = 0usize;
            for k in 0..inner.len() {
                if matches!(inner[k].kind, TokenKind::Dollar) {
                    if let Some(TokenKind::Ident(name)) = inner.get(k + 1).map(|t| &t.kind) {
                        if let Some(list) = rep_bindings.get(name) {
                            n = list.len();
                            break;
                        }
                    }
                }
            }
            for rep_idx in 0..n {
                if rep_idx > 0 {
                    if let Some(ref s) = sep {
                        out.push(s.clone());
                    }
                }
                let mut iter_bindings = bindings.clone();
                for (k, list) in rep_bindings {
                    if let Some(item) = list.get(rep_idx) {
                        iter_bindings.insert(k.clone(), item.clone());
                    }
                }
                out.extend(substitute_body(inner, &iter_bindings, rep_bindings));
            }
            i = next_i;
            continue;
        }
        if matches!(body[i].kind, TokenKind::Dollar) {
            if let Some(TokenKind::Ident(var)) = body.get(i + 1).map(|t| &t.kind) {
                if let Some(val) = bindings.get(var) {
                    out.extend(val.iter().cloned());
                    i += 2;
                    continue;
                }
            }
        }
        out.push(body[i].clone());
        i += 1;
    }
    out
}
