//! PHP expression lowering to ZIPP register bytecode.

use super::emitter::{PhpEmitter, R, MAX_DEPTH, REG_ARGS, REG_CALLED, REG_SCOPE, REG_THIS};
use super::stmts::peel_index_chain;
use super::symtable::{arrow_captures, is_known_by_val_builtin, known_ref_arg_mask};
use crate::bytecode::{Instr, Reg};
use std::sync::atomic::{AtomicU64, Ordering};
use zipp_phpparse::ast::*;

/// `TYPEOF_NAMES` indices from `crate::bytecode::TYPEOF_NAMES`.
const TY_NUMBER: u8 = 0;
const TY_STRING: u8 = 1;
const TY_BOOL: u8 = 2;
const TY_UNDEFINED: u8 = 3;
const TY_OBJECT: u8 = 4;
const TY_BIGINT: u8 = 7;

/// Safe unboxed `small_bigint` magnitude bound (`2^45` = 35 trillion, well inside `SMALL_BIGINT` `±2^46` and PHP `i64`).
/// Any value outside `[-SAFE_SMALL_INT, SAFE_SMALL_INT]` falls back cleanly to the exact PHP 64-bit/float runtime helper.
const SAFE_SMALL_INT: i64 = 1i64 << 45;

/// True when `expr` is statically guaranteed to produce a JS `boolean` (`true` / `false`).
pub(super) fn is_known_bool(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Bool(_)
        | ExprKind::Isset(_)
        | ExprKind::Empty(_)
        | ExprKind::InstanceOf { .. } => true,
        ExprKind::Unary {
            op: UnaryOp::Not, ..
        } => true,
        ExprKind::Cast {
            kind: CastKind::Bool,
            ..
        } => true,
        ExprKind::Binary { op, .. } => matches!(
            op,
            BinOp::Eq
                | BinOp::NotEq
                | BinOp::StrictEq
                | BinOp::StrictNotEq
                | BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::LogAnd
                | BinOp::LogOr
                | BinOp::LogXor
        ),
        _ => false,
    }
}

/// True when `expr` is statically guaranteed NOT to produce a `PhpArray` (or `PhpObject`),
/// so storing it into a local never needs `R.share()`, and `===`/`!==` against it never needs deep array compare.
pub(super) fn is_known_scalar(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Null
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::InterpString(_)
        | ExprKind::MagicConst(_)
        | ExprKind::Isset(_)
        | ExprKind::Empty(_)
        | ExprKind::Print(_)
        | ExprKind::InstanceOf { .. } => true,
        ExprKind::Unary { op, .. } => !matches!(op, UnaryOp::Suppress),
        ExprKind::Cast { kind, .. } => !matches!(kind, CastKind::Array | CastKind::Object),
        ExprKind::Binary { op, left, right } => match op {
            BinOp::Coalesce => is_known_scalar(left) && is_known_scalar(right),
            BinOp::Add => is_known_scalar(left) || is_known_scalar(right),
            _ => true,
        },
        ExprKind::Call {
            callee: Callee::Name(n),
            ..
        } if n.parts.len() == 1 => {
            let lo = n.parts[0].to_ascii_lowercase();
            matches!(
                lo.as_str(),
                "strlen"
                    | "count"
                    | "sizeof"
                    | "strpos"
                    | "stripos"
                    | "strrpos"
                    | "substr"
                    | "strtolower"
                    | "strtoupper"
                    | "trim"
                    | "ltrim"
                    | "rtrim"
                    | "implode"
                    | "join"
                    | "sprintf"
                    | "json_encode"
                    | "htmlspecialchars"
                    | "urlencode"
                    | "rawurlencode"
                    | "base64_encode"
                    | "base64_decode"
                    | "md5"
                    | "sha1"
                    | "crc32"
                    | "hash"
                    | "hash_hmac"
                    | "hash_equals"
                    | "time"
                    | "microtime"
                    | "intval"
                    | "floatval"
                    | "strval"
                    | "boolval"
                    | "is_array"
                    | "is_string"
                    | "is_int"
                    | "is_float"
                    | "is_bool"
                    | "is_null"
                    | "is_object"
                    | "is_numeric"
                    | "is_scalar"
                    | "is_callable"
                    | "array_key_exists"
                    | "in_array"
                    | "str_starts_with"
                    | "str_ends_with"
                    | "str_contains"
                    | "mb_strlen"
                    | "class_exists"
                    | "function_exists"
                    | "method_exists"
                    | "file_exists"
            )
        }
        _ => false,
    }
}

/// True when `expr` produces a freshly-owned value (known scalar OR brand-new array literal)
/// that cannot alias any existing variable, so `store_var_owned` is 100% sound.
pub(super) fn is_fresh_owned(expr: &Expr) -> bool {
    is_known_scalar(expr) || matches!(expr.kind, ExprKind::Array(_))
}

static ANON_CLASS_SEQ: AtomicU64 = AtomicU64::new(1);

impl<'a> PhpEmitter<'a> {
    pub fn emit_expr(&mut self, expr: &Expr, depth: usize) -> R<Reg> {
        if depth > MAX_DEPTH {
            return Err("PHP: expression nesting limit exceeded".into());
        }
        match &expr.kind {
            ExprKind::Null => self.null(),
            ExprKind::Bool(b) => self.boolean(*b),
            ExprKind::Int(v) => self.int64(*v),
            ExprKind::Float(v) => self.float(*v),
            ExprKind::String(s) => self.string(s),
            ExprKind::InterpString(parts) => {
                if parts.is_empty() {
                    return self.string("");
                }
                let mut acc: Option<Reg> = None;
                for p in parts {
                    let (seg, is_lit_str, is_int_lit) = match p {
                        StringPart::Literal(s) => (self.string(s)?, true, false),
                        StringPart::Expr(e) => {
                            let is_str = matches!(e.kind, ExprKind::String(_) | ExprKind::InterpString(_));
                            let is_int = matches!(e.kind, ExprKind::Int(_));
                            (self.emit_expr(e, depth + 1)?, is_str, is_int)
                        }
                    };
                    acc = Some(match acc {
                        None => {
                            if is_lit_str {
                                seg
                            } else {
                                let cast_str = self.small_int(2)?;
                                self.helper("cast", &[cast_str, seg])?
                            }
                        }
                        Some(prev) => self.emit_fast_concat(prev, true, seg, is_lit_str, is_int_lit)?,
                    });
                }
                Ok(acc.unwrap())
            }
            ExprKind::MagicConst(mc) => match mc {
                MagicConst::File => {
                    let s = self.file.file.clone();
                    self.string(&s)
                }
                MagicConst::Dir => {
                    let s = self.file.dir.clone();
                    self.string(&s)
                }
                MagicConst::Line => self.int64(self.file.line_of(expr.span.start)),
                MagicConst::Function => {
                    let s = self.fn_name.clone();
                    self.string(&s)
                }
                MagicConst::Class => {
                    let s = self.class_name.clone().unwrap_or_default();
                    self.string(&s)
                }
                MagicConst::Trait => {
                    let s = self.trait_name.clone().unwrap_or_default();
                    self.string(&s)
                }
                MagicConst::Method => {
                    let s = match (&self.class_name, self.fn_name.is_empty()) {
                        (Some(c), false) => format!("{c}::{}", self.fn_name),
                        _ => self.fn_name.clone(),
                    };
                    self.string(&s)
                }
                MagicConst::Namespace => {
                    let s = self.ns.namespace.clone();
                    self.string(&s)
                }
            },
            ExprKind::Var(name) => self.load_var(name, false),
            ExprKind::DynamicVar(inner) => {
                let n = self.emit_expr(inner, depth + 1)?;
                let scope = self.scope_map_reg()?;
                let q = self.boolean(false)?;
                self.helper("sget", &[scope, n, q])
            }
            ExprKind::ConstFetch(name) => {
                let (fqn, fb) = self.ns.resolve_const(name);
                let fr = self.string(&fqn)?;
                let fbr = match fb {
                    Some(g) => self.string(&g)?,
                    None => self.null()?,
                };
                self.helper("getConst", &[fr, fbr])
            }
            ExprKind::Array(items) => {
                let arr = self.helper("arr", &[])?;
                for it in items {
                    let mark = self.mark();
                    if it.unpack {
                        let v = self.emit_expr(&it.value, depth + 1)?;
                        self.helper("arrSpread", &[arr, v])?;
                    } else if it.by_ref {
                        let k = match &it.key {
                            Some(ke) => self.emit_expr(ke, depth + 1)?,
                            None => self.undefined()?,
                        };
                        let r = self.emit_lvalue_ref(&it.value, depth + 1)?;
                        self.helper("arrRefLit", &[arr, k, r])?;
                    } else if let Some(ke) = &it.key {
                        let k = self.emit_expr(ke, depth + 1)?;
                        let v = self.emit_expr(&it.value, depth + 1)?;
                        self.helper("arrSetLit", &[arr, k, v])?;
                    } else {
                        let v = self.emit_expr(&it.value, depth + 1)?;
                        self.helper("arrPush", &[arr, v])?;
                    }
                    self.release(mark);
                }
                Ok(arr)
            }
            ExprKind::List(_) => Err("PHP: list() is only valid as an assignment target".into()),

            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Plus => {
                    let v = self.emit_expr(inner, depth + 1)?;
                    self.helper("uplus", &[v])
                }
                UnaryOp::Neg => {
                    let v = self.emit_expr(inner, depth + 1)?;
                    self.helper("uneg", &[v])
                }
                UnaryOp::Not => {
                    let v = self.emit_cond_bool(inner, depth + 1)?;
                    let dst = self.alloc()?;
                    self.emit(Instr::Not { dst, a: v })?;
                    Ok(dst)
                }
                UnaryOp::BitNot => {
                    let v = self.emit_expr(inner, depth + 1)?;
                    self.helper("bnot", &[v])
                }
                UnaryOp::Suppress => {
                    self.helper("supPush", &[])?;
                    let res = self.emit_expr(inner, depth + 1)?;
                    self.helper("supPop", &[])?;
                    Ok(res)
                }
                UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
                    let cur = self.emit_expr(inner, depth + 1)?;
                    let is_inc = matches!(op, UnaryOp::PreInc | UnaryOp::PostInc);
                    let next = self.emit_fast_inc_dec(cur, is_inc)?;
                    self.emit_assign_target_owned(inner, next, depth + 1)?;
                    if matches!(op, UnaryOp::PostInc | UnaryOp::PostDec) {
                        Ok(cur)
                    } else {
                        Ok(next)
                    }
                }
            },
            ExprKind::Cast { kind, expr: inner } => {
                let v = self.emit_expr(inner, depth + 1)?;
                let code = match kind {
                    CastKind::Int => 0,
                    CastKind::Float => 1,
                    CastKind::String => 2,
                    CastKind::Bool => 3,
                    CastKind::Array => 4,
                    CastKind::Object => 5,
                    CastKind::Unset => 6,
                };
                let k = self.small_int(code)?;
                self.helper("cast", &[k, v])
            }
            ExprKind::Binary { op, left, right } => match op {
                BinOp::LogAnd => {
                    let dst = self.alloc()?;
                    let lb = self.emit_cond_bool(left, depth + 1)?;
                    self.emit(Instr::Move { dst, src: lb })?;
                    let skip = self.jump_if_false(lb)?;
                    let rb = self.emit_cond_bool(right, depth + 1)?;
                    self.emit(Instr::Move { dst, src: rb })?;
                    let end = self.here();
                    self.patch(skip, end)?;
                    Ok(dst)
                }
                BinOp::LogOr => {
                    let dst = self.alloc()?;
                    let lb = self.emit_cond_bool(left, depth + 1)?;
                    self.emit(Instr::Move { dst, src: lb })?;
                    let skip = self.jump_if_true(lb)?;
                    let rb = self.emit_cond_bool(right, depth + 1)?;
                    self.emit(Instr::Move { dst, src: rb })?;
                    let end = self.here();
                    self.patch(skip, end)?;
                    Ok(dst)
                }
                BinOp::LogXor => {
                    let lb = self.emit_cond_bool(left, depth + 1)?;
                    let rb = self.emit_cond_bool(right, depth + 1)?;
                    let dst = self.alloc()?;
                    self.emit(Instr::Ne { dst, a: lb, b: rb })?;
                    Ok(dst)
                }
                BinOp::Coalesce => {
                    let dst = self.alloc()?;
                    let is_set = self.emit_isset_one(left, depth + 1)?;
                    let use_rhs = self.jump_if_false(is_set)?;
                    let lv = self.emit_expr_quiet(left, depth + 1)?;
                    self.emit(Instr::Move { dst, src: lv })?;
                    let done = self.jump()?;
                    let rhs_at = self.here();
                    self.patch(use_rhs, rhs_at)?;
                    let rv = self.emit_expr(right, depth + 1)?;
                    self.emit(Instr::Move { dst, src: rv })?;
                    let end = self.here();
                    self.patch(done, end)?;
                    Ok(dst)
                }
                BinOp::StrictEq | BinOp::StrictNotEq => {
                    let lv = self.emit_expr(left, depth + 1)?;
                    let rv = self.emit_expr(right, depth + 1)?;
                    let is_eq = *op == BinOp::StrictEq;
                    if is_known_scalar(left) || is_known_scalar(right) {
                        let dst = self.alloc()?;
                        self.emit(if is_eq {
                            Instr::Eq { dst, a: lv, b: rv }
                        } else {
                            Instr::Ne { dst, a: lv, b: rv }
                        })?;
                        return Ok(dst);
                    }
                    self.emit_fast_strict_eq(lv, rv, is_eq)
                }
                BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                    let lv = self.emit_expr(left, depth + 1)?;
                    let rv = self.emit_expr(right, depth + 1)?;
                    let l_int = matches!(left.kind, ExprKind::Int(_));
                    let r_int = matches!(right.kind, ExprKind::Int(_));
                    self.emit_fast_int_cmp(*op, lv, l_int, rv, r_int)
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    let lv = self.emit_expr(left, depth + 1)?;
                    let l_int = matches!(left.kind, ExprKind::Int(_));
                    // Fast immediate `x + k` / `x - k` for small positive literals (`$n - 1`, `$n - 2`, `$i + 1`)
                    if matches!(op, BinOp::Add | BinOp::Sub) {
                        if let ExprKind::Int(k) = right.kind {
                            if (1..=1_000_000).contains(&k) {
                                let imm = if *op == BinOp::Add { k as i32 } else { -(k as i32) };
                                return self.emit_fast_add_imm(lv, l_int, imm);
                            }
                        }
                    }
                    let rv = self.emit_expr(right, depth + 1)?;
                    let r_int = matches!(right.kind, ExprKind::Int(_));
                    self.emit_fast_int_arith(*op, lv, l_int, rv, r_int)
                }
                BinOp::Concat => {
                    // Flatten left-nested `a . b . c . ...` chains iteratively to avoid deep
                    // recursion and reuse temp registers on huge generated concatenations!
                    let mut parts: Vec<&Expr> = vec![right];
                    let mut cur: &Expr = left;
                    while let ExprKind::Binary {
                        op: BinOp::Concat,
                        left: l2,
                        right: r2,
                    } = &cur.kind
                    {
                        parts.push(r2);
                        cur = l2;
                    }
                    parts.push(cur);
                    parts.reverse();
                    let acc = self.alloc()?;
                    let first = self.emit_expr(parts[0], depth + 1)?;
                    self.emit(Instr::Move { dst: acc, src: first })?;
                    let mut acc_is_str =
                        matches!(parts[0].kind, ExprKind::String(_) | ExprKind::InterpString(_));
                    for seg_expr in &parts[1..] {
                        let mark = self.mark();
                        let rv = self.emit_expr(seg_expr, depth + 1)?;
                        let r_str = matches!(
                            seg_expr.kind,
                            ExprKind::String(_) | ExprKind::InterpString(_)
                        );
                        let r_int = matches!(seg_expr.kind, ExprKind::Int(_));
                        let next = self.emit_fast_concat(acc, acc_is_str, rv, r_str, r_int)?;
                        self.emit(Instr::Move { dst: acc, src: next })?;
                        acc_is_str = true;
                        self.release(mark);
                    }
                    Ok(acc)
                }
                _ => {
                    let lv = self.emit_expr(left, depth + 1)?;
                    let rv = self.emit_expr(right, depth + 1)?;
                    let h = match op {
                        BinOp::Div => "div",
                        BinOp::Mod => "mod",
                        BinOp::Pow => "pow",
                        BinOp::BitAnd => "band",
                        BinOp::BitOr => "bor",
                        BinOp::BitXor => "bxor",
                        BinOp::Shl => "shl",
                        BinOp::Shr => "shr",
                        BinOp::Eq => "eq",
                        BinOp::NotEq => "neq",
                        BinOp::Spaceship => "cmp",
                        _ => unreachable!(),
                    };
                    self.helper(h, &[lv, rv])
                }
            },
            ExprKind::Ternary {
                cond,
                then_expr,
                else_expr,
            } => {
                let dst = self.alloc()?;
                let (cv, cb) = if then_expr.is_some() {
                    let b = self.emit_cond_bool(cond, depth + 1)?;
                    (b, b)
                } else {
                    let v = self.emit_expr(cond, depth + 1)?;
                    let b = if is_known_bool(cond) { v } else { self.truthy(v)? };
                    (v, b)
                };
                let to_else = self.jump_if_false(cb)?;
                let tv = match then_expr {
                    Some(te) => self.emit_expr(te, depth + 1)?,
                    None => cv,
                };
                self.emit(Instr::Move { dst, src: tv })?;
                let done = self.jump()?;
                let else_at = self.here();
                self.patch(to_else, else_at)?;
                let ev = self.emit_expr(else_expr, depth + 1)?;
                self.emit(Instr::Move { dst, src: ev })?;
                let end = self.here();
                self.patch(done, end)?;
                Ok(dst)
            }
            ExprKind::Assign { op, target, value } => match op {
                AssignOp::Assign => {
                    let v = self.emit_expr(value, depth + 1)?;
                    if is_fresh_owned(value) {
                        self.emit_assign_target_owned(target, v, depth + 1)
                    } else {
                        self.emit_assign_target(target, v, depth + 1)
                    }
                }
                AssignOp::AssignRef => {
                    let r = self.emit_lvalue_ref(value, depth + 1)?;
                    self.emit_assign_ref_target(target, r, depth + 1)?;
                    self.helper("deref", &[r])
                }
                AssignOp::Coalesce => {
                    let dst = self.alloc()?;
                    let is_set = self.emit_isset_one(target, depth + 1)?;
                    let do_assign = self.jump_if_false(is_set)?;
                    let cur = self.emit_expr_quiet(target, depth + 1)?;
                    self.emit(Instr::Move { dst, src: cur })?;
                    let done = self.jump()?;
                    let a_at = self.here();
                    self.patch(do_assign, a_at)?;
                    let v = self.emit_expr(value, depth + 1)?;
                    let stored = if is_fresh_owned(value) {
                        self.emit_assign_target_owned(target, v, depth + 1)?
                    } else {
                        self.emit_assign_target(target, v, depth + 1)?
                    };
                    self.emit(Instr::Move { dst, src: stored })?;
                    let end = self.here();
                    self.patch(done, end)?;
                    Ok(dst)
                }
                AssignOp::Add | AssignOp::Sub | AssignOp::Mul => {
                    let cur = self.emit_expr(target, depth + 1)?;
                    let bop = match op {
                        AssignOp::Add => BinOp::Add,
                        AssignOp::Sub => BinOp::Sub,
                        _ => BinOp::Mul,
                    };
                    let next = if matches!(op, AssignOp::Add | AssignOp::Sub) {
                        if let ExprKind::Int(k) = value.kind {
                            if (1..=1_000_000).contains(&k) {
                                let imm = if *op == AssignOp::Add { k as i32 } else { -(k as i32) };
                                self.emit_fast_add_imm(cur, false, imm)?
                            } else {
                                let rhs = self.emit_expr(value, depth + 1)?;
                                self.emit_fast_int_arith(bop, cur, false, rhs, true)?
                            }
                        } else {
                            let rhs = self.emit_expr(value, depth + 1)?;
                            self.emit_fast_int_arith(bop, cur, false, rhs, false)?
                        }
                    } else {
                        let rhs = self.emit_expr(value, depth + 1)?;
                        let r_int = matches!(value.kind, ExprKind::Int(_));
                        self.emit_fast_int_arith(bop, cur, false, rhs, r_int)?
                    };
                    // `R.add`/`R.sub`/`R.mul` always produce a scalar or fresh array union -> owned!
                    self.emit_assign_target_owned(target, next, depth + 1)
                }
                AssignOp::Concat => {
                    let cur = self.emit_expr(target, depth + 1)?;
                    let rhs = self.emit_expr(value, depth + 1)?;
                    let r_str = matches!(value.kind, ExprKind::String(_) | ExprKind::InterpString(_));
                    let r_int = matches!(value.kind, ExprKind::Int(_));
                    let next = self.emit_fast_concat(cur, false, rhs, r_str, r_int)?;
                    self.emit_assign_target_owned(target, next, depth + 1)
                }
                _ => {
                    let cur = self.emit_expr(target, depth + 1)?;
                    let rhs = self.emit_expr(value, depth + 1)?;
                    let h = match op {
                        AssignOp::Div => "div",
                        AssignOp::Mod => "mod",
                        AssignOp::Pow => "pow",
                        AssignOp::BitAnd => "band",
                        AssignOp::BitOr => "bor",
                        AssignOp::BitXor => "bxor",
                        AssignOp::Shl => "shl",
                        AssignOp::Shr => "shr",
                        _ => unreachable!(),
                    };
                    let next = self.helper(h, &[cur, rhs])?;
                    self.emit_assign_target_owned(target, next, depth + 1)
                }
            },
            ExprKind::Index { target, index } => {
                let Some(idx_expr) = index else {
                    return Err("PHP: cannot use [] for reading".into());
                };
                let t = self.emit_expr(target, depth + 1)?;
                let k = self.emit_expr(idx_expr, depth + 1)?;
                let q = self.boolean(false)?;
                self.helper("idxGet", &[t, k, q])
            }
            ExprKind::Prop {
                target,
                prop,
                nullsafe,
            } => {
                // Fast inline `$this->prop` read via monomorphic `REG_THIS.fp.<prop>` (zero JS calls!).
                if !*nullsafe && self.class_name.is_some() {
                    if let (ExprKind::Var(vname), PropName::Ident(pname)) = (&target.kind, prop) {
                        if vname == "this" {
                            let dst = self.alloc()?;
                            let fp = self.prop(REG_THIS, "fp")?;
                            let j_slow1 = self.jump_if_false(fp)?;
                            let v = self.prop(fp, pname)?;
                            let is_u = self.type_of_is(v, TY_UNDEFINED, false)?;
                            let j_slow2 = self.jump_if_true(is_u)?;
                            self.emit(Instr::Move { dst, src: v })?;
                            let j_done = self.jump()?;
                            let slow = self.here();
                            self.patch(j_slow1, slow)?;
                            self.patch(j_slow2, slow)?;
                            let pn = self.string(pname)?;
                            let ns = self.boolean(false)?;
                            let sc = self.scope_cls_reg()?;
                            let q = self.boolean(false)?;
                            let sv = self.helper("propGet", &[REG_THIS, pn, ns, sc, q])?;
                            self.emit(Instr::Move { dst, src: sv })?;
                            let end = self.here();
                            self.patch(j_done, end)?;
                            return Ok(dst);
                        }
                    }
                }
                let obj = self.emit_expr(target, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let ns = self.boolean(*nullsafe)?;
                let sc = self.scope_cls_reg()?;
                let q = self.boolean(false)?;
                self.helper("propGet", &[obj, pn, ns, sc, q])
            }
            ExprKind::StaticProp { class, prop } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("staticGet", &[cr, pn, sc, REG_CALLED])
            }
            ExprKind::ClassConst { class, name } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let nr = self.emit_prop_name(name, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("staticConst", &[cr, nr, sc, REG_CALLED])
            }
            ExprKind::Call { callee, args } => {
                // Scope-aware special builtins: `compact`, `extract`, `get_defined_vars`
                if let Callee::Name(n) = callee {
                    if n.parts.len() == 1 {
                        let lo = n.parts[0].to_ascii_lowercase();
                        if lo == "compact" {
                            if let CallArgs::Normal(list) = args {
                                let scope = self.scope_map_reg()?;
                                let mut nregs = Vec::new();
                                for a in list {
                                    nregs.push(self.emit_expr(&a.value, depth + 1)?);
                                }
                                let arr = self.js_array(&nregs)?;
                                return self.helper("compact", &[scope, arr]);
                            }
                        } else if lo == "extract" {
                            if let CallArgs::Normal(list) = args {
                                if let Some(first) = list.first() {
                                    let scope = self.scope_map_reg()?;
                                    let arr = self.emit_expr(&first.value, depth + 1)?;
                                    return self.helper("extract", &[scope, arr]);
                                }
                            }
                        } else if lo == "get_defined_vars" {
                            let scope = self.scope_map_reg()?;
                            return self.helper("getDefinedVars", &[scope]);
                        } else if lo == "func_get_args" {
                            return self.helper("funcGetArgs", &[REG_ARGS]);
                        } else if lo == "func_num_args" {
                            return self.helper("funcNumArgs", &[REG_ARGS]);
                        } else if lo == "func_get_arg" {
                            if let CallArgs::Normal(list) = args {
                                if let Some(first) = list.first() {
                                    let idx = self.emit_expr(&first.value, depth + 1)?;
                                    return self.helper("funcGetArg", &[REG_ARGS, idx]);
                                }
                            }
                        } else if lo == "get_called_class" {
                            return self.helper("calledClassName", &[REG_CALLED]);
                        }
                    }
                    // Direct bytecode intrinsics for hot global builtins (only when statically guaranteed global!).
                    if let CallArgs::Normal(list) = args {
                        if list.len() == 1 && !list[0].unpack && list[0].name.is_none() {
                            let (fqn, fb) = self.ns.resolve_function(n);
                            if fb.is_none() && !fqn.contains('\\') {
                                match fqn.to_ascii_lowercase().as_str() {
                                    "is_int" | "is_integer" | "is_long" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.type_of_is(v, TY_BIGINT, false);
                                    }
                                    "is_string" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.type_of_is(v, TY_STRING, false);
                                    }
                                    "is_bool" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.type_of_is(v, TY_BOOL, false);
                                    }
                                    "is_float" | "is_double" | "is_real" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.type_of_is(v, TY_NUMBER, false);
                                    }
                                    "is_null" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        let nr = self.null()?;
                                        let dst = self.alloc()?;
                                        self.emit(Instr::Eq { dst, a: v, b: nr })?;
                                        return Ok(dst);
                                    }
                                    "is_array" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.helper("isArr", &[v]);
                                    }
                                    "strlen" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.helper("strlen", &[v]);
                                    }
                                    "count" | "sizeof" => {
                                        let v = self.emit_expr(&list[0].value, depth + 1)?;
                                        return self.helper("count", &[v]);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                let sc = self.scope_cls_reg()?;
                let st = self.boolean(self.file.strict_types)?;
                match args {
                    CallArgs::FirstClassCallable => {
                        let target = match callee {
                            Callee::Name(n) => {
                                let (fqn, _) = self.ns.resolve_function(n);
                                self.string(&fqn)?
                            }
                            Callee::Expr(e) => self.emit_expr(e, depth + 1)?,
                        };
                        let none = self.null()?;
                        let is_st = self.boolean(false)?;
                        self.helper("firstClass", &[target, none, is_st, REG_CALLED, sc])
                    }
                    CallArgs::Normal(list) => {
                        let ref_mask = match callee {
                            Callee::Name(n) if n.parts.len() == 1 => known_ref_arg_mask(&n.parts[0]),
                            _ => 0,
                        };
                        let by_val_only = ref_mask != 0
                            || matches!(
                                callee,
                                Callee::Name(n) if n.parts.len() == 1 && is_known_by_val_builtin(&n.parts[0])
                            );
                        let simple_pos = list.len() <= 2
                            && list.iter().all(|a| !a.unpack && a.name.is_none());
                        match callee {
                            Callee::Name(n) if simple_pos => {
                                let (fqn, fb) = self.ns.resolve_function(n);
                                let lo_fqn = fqn.trim_start_matches('\\').to_ascii_lowercase();
                                let fr = self.string(&lo_fqn)?;
                                let fbr = match fb {
                                    Some(g) => {
                                        let lo_g = g.trim_start_matches('\\').to_ascii_lowercase();
                                        self.string(&lo_g)?
                                    }
                                    None => self.null()?,
                                };
                                match list.len() {
                                    0 => self.helper("callName0", &[fr, fbr, sc, st]),
                                    1 => {
                                        let a0 = self.emit_one_call_arg_ex(&list[0], 0, by_val_only, ref_mask, depth + 1)?;
                                        self.helper("callName1", &[fr, fbr, a0, sc, st])
                                    }
                                    2 => {
                                        let a0 = self.emit_one_call_arg_ex(&list[0], 0, by_val_only, ref_mask, depth + 1)?;
                                        let a1 = self.emit_one_call_arg_ex(&list[1], 1, by_val_only, ref_mask, depth + 1)?;
                                        self.helper("callName2", &[fr, fbr, a0, a1, sc, st])
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            Callee::Name(n) => {
                                let (pos_arr, named_obj) =
                                    self.emit_call_args_pair_mask(list, by_val_only, ref_mask, depth + 1)?;
                                let (fqn, fb) = self.ns.resolve_function(n);
                                let fr = self.string(&fqn)?;
                                let fbr = match fb {
                                    Some(g) => self.string(&g)?,
                                    None => self.null()?,
                                };
                                self.helper("callName", &[fr, fbr, pos_arr, named_obj, sc, st])
                            }
                            Callee::Expr(e) => {
                                let (pos_arr, named_obj) =
                                    self.emit_call_args_pair_ex(list, false, depth + 1)?;
                                let cv = self.emit_expr(e, depth + 1)?;
                                self.helper("call", &[cv, pos_arr, named_obj, sc, st])
                            }
                        }
                    }
                }
            }
            ExprKind::MethodCall {
                target,
                method,
                nullsafe,
                args,
            } => {
                let obj = self.emit_expr(target, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                match args {
                    CallArgs::FirstClassCallable => {
                        let mn = self.emit_prop_name(method, depth + 1)?;
                        let is_st = self.boolean(false)?;
                        self.helper("firstClass", &[obj, mn, is_st, REG_CALLED, sc])
                    }
                    CallArgs::Normal(list) => {
                        let st = self.boolean(self.file.strict_types)?;
                        if *nullsafe {
                            // Short-circuit evaluation of arguments when `$obj === null`!
                            let mn = self.emit_prop_name(method, depth + 1)?;
                            let ns = self.boolean(true)?;
                            let dst = self.alloc()?;
                            let null_r = self.null()?;
                            let is_null = self.alloc()?;
                            self.emit(Instr::Eq {
                                dst: is_null,
                                a: obj,
                                b: null_r,
                            })?;
                            let not_null = self.jump_if_false(is_null)?;
                            self.emit(Instr::Move { dst, src: null_r })?;
                            let done = self.jump()?;
                            let call_at = self.here();
                            self.patch(not_null, call_at)?;
                            let (pos_arr, named_obj) = self.emit_call_args_pair(list, depth + 1)?;
                            let res =
                                self.helper("methodCall", &[obj, mn, ns, pos_arr, named_obj, sc, st])?;
                            self.emit(Instr::Move { dst, src: res })?;
                            let end = self.here();
                            self.patch(done, end)?;
                            Ok(dst)
                        } else if let PropName::Ident(mname) = method {
                            let simple_pos = list.len() <= 2
                                && list.iter().all(|a| !a.unpack && a.name.is_none());
                            if simple_pos {
                                let lo = mname.to_ascii_lowercase();
                                let can_inline_call = self.class_name.is_none()
                                    || !self.class_private_methods.contains(&lo);
                                match list.len() {
                                    0 if can_inline_call => {
                                        // Inline monomorphic call-site fast path for 0-arg public direct methods (`m.f0`)
                                        let dst = self.alloc()?;
                                        let j_s0 = self.jump_if_false(obj)?;
                                        let is_obj = self.type_of_is(obj, TY_OBJECT, false)?;
                                        let j_s1 = self.jump_if_false(is_obj)?;
                                        let cls = self.prop(obj, "cls")?;
                                        let j_s2 = self.jump_if_false(cls)?;
                                        let mobj = self.prop(cls, "mObj")?;
                                        let j_s3 = self.jump_if_false(mobj)?;
                                        let m = self.prop(mobj, &lo)?;
                                        let j_s4 = self.jump_if_false(m)?;
                                        let f0 = self.prop(m, "f0")?;
                                        let j_s5 = self.jump_if_false(f0)?;
                                        let base = self.block(3)?;
                                        self.emit(Instr::LoadNull { dst: base })?;
                                        self.emit(Instr::Move { dst: base + 1, src: obj })?;
                                        self.emit(Instr::Move { dst: base + 2, src: cls })?;
                                        self.emit(Instr::CallWithThis {
                                            dst,
                                            callee: f0,
                                            this_v: m,
                                            arg_base: base,
                                            argc: 3,
                                            name: crate::bytecode::NO_NAME,
                                        })?;
                                        let j_done = self.jump()?;
                                        let slow = self.here();
                                        for j in [j_s0, j_s1, j_s2, j_s3, j_s4, j_s5] {
                                            self.patch(j, slow)?;
                                        }
                                        let mn = self.string(&lo)?;
                                        let sr = self.helper("mCall0", &[obj, mn, sc, st])?;
                                        self.emit(Instr::Move { dst, src: sr })?;
                                        let end = self.here();
                                        self.patch(j_done, end)?;
                                        Ok(dst)
                                    }
                                    0 => {
                                        let mn = self.string(&lo)?;
                                        self.helper("mCall0", &[obj, mn, sc, st])
                                    }
                                    1 if can_inline_call => {
                                        // Inline monomorphic call-site fast path for 1-arg `int -> int` public direct methods (`m.fi1`)
                                        let a0_is_ref_var = matches!(
                                            &list[0].value.kind,
                                            ExprKind::Var(v) if v != "this" && matches!(self.vars.get(v), Some(super::emitter::VarSlot::Ref(_)))
                                        );
                                        let a0_raw = self.emit_one_call_arg(&list[0], false, depth + 1)?;
                                        let dst = self.alloc()?;
                                        let j_s0 = self.jump_if_false(obj)?;
                                        let is_obj = self.type_of_is(obj, TY_OBJECT, false)?;
                                        let j_s1 = self.jump_if_false(is_obj)?;
                                        let cls = self.prop(obj, "cls")?;
                                        let j_s2 = self.jump_if_false(cls)?;
                                        let mobj = self.prop(cls, "mObj")?;
                                        let j_s3 = self.jump_if_false(mobj)?;
                                        let m = self.prop(mobj, &lo)?;
                                        let j_s4 = self.jump_if_false(m)?;
                                        let fi1 = self.prop(m, "fi1")?;
                                        let j_s5 = self.jump_if_false(fi1)?;
                                        let a0_val = if a0_is_ref_var {
                                            self.prop(a0_raw, "v")?
                                        } else {
                                            a0_raw
                                        };
                                        let is_bi = self.type_of_is(a0_val, TY_BIGINT, false)?;
                                        let j_s6 = self.jump_if_false(is_bi)?;
                                        let base = self.block(3)?;
                                        self.emit(Instr::Move { dst: base, src: a0_val })?;
                                        self.emit(Instr::Move { dst: base + 1, src: obj })?;
                                        self.emit(Instr::Move { dst: base + 2, src: cls })?;
                                        self.emit(Instr::CallWithThis {
                                            dst,
                                            callee: fi1,
                                            this_v: m,
                                            arg_base: base,
                                            argc: 3,
                                            name: crate::bytecode::NO_NAME,
                                        })?;
                                        let ret_bi = self.type_of_is(dst, TY_BIGINT, false)?;
                                        let j_ok = self.jump_if_true(ret_bi)?;
                                        let checked = self.helper("checkRet", &[dst, m, cls])?;
                                        self.emit(Instr::Move { dst, src: checked })?;
                                        let j_done = self.jump()?;
                                        let slow = self.here();
                                        for j in [j_s0, j_s1, j_s2, j_s3, j_s4, j_s5, j_s6] {
                                            self.patch(j, slow)?;
                                        }
                                        let mn = self.string(&lo)?;
                                        let sr = self.helper("mCall1", &[obj, mn, a0_raw, sc, st])?;
                                        self.emit(Instr::Move { dst, src: sr })?;
                                        let end = self.here();
                                        self.patch(j_ok, end)?;
                                        self.patch(j_done, end)?;
                                        Ok(dst)
                                    }
                                    1 => {
                                        let mn = self.string(&lo)?;
                                        let a0 = self.emit_one_call_arg(&list[0], false, depth + 1)?;
                                        self.helper("mCall1", &[obj, mn, a0, sc, st])
                                    }
                                    2 => {
                                        let mn = self.string(&lo)?;
                                        let a0 = self.emit_one_call_arg(&list[0], false, depth + 1)?;
                                        let a1 = self.emit_one_call_arg(&list[1], false, depth + 1)?;
                                        self.helper("mCall2", &[obj, mn, a0, a1, sc, st])
                                    }
                                    _ => unreachable!(),
                                }
                            } else {
                                let mn = self.string(mname)?;
                                let ns = self.boolean(false)?;
                                let (pos_arr, named_obj) = self.emit_call_args_pair(list, depth + 1)?;
                                self.helper("methodCall", &[obj, mn, ns, pos_arr, named_obj, sc, st])
                            }
                        } else {
                            let mn = self.emit_prop_name(method, depth + 1)?;
                            let ns = self.boolean(false)?;
                            let (pos_arr, named_obj) = self.emit_call_args_pair(list, depth + 1)?;
                            self.helper("methodCall", &[obj, mn, ns, pos_arr, named_obj, sc, st])
                        }
                    }
                }
            }
            ExprKind::StaticCall {
                class,
                method,
                args,
            } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let mn = self.emit_prop_name(method, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                match args {
                    CallArgs::FirstClassCallable => {
                        let is_st = self.boolean(true)?;
                        self.helper("firstClass", &[cr, mn, is_st, REG_CALLED, sc])
                    }
                    CallArgs::Normal(list) => {
                        let (pos_arr, named_obj) = self.emit_call_args_pair(list, depth + 1)?;
                        let st = self.boolean(self.file.strict_types)?;
                        self.helper(
                            "staticCall",
                            &[cr, mn, pos_arr, named_obj, REG_THIS, sc, REG_CALLED, st],
                        )
                    }
                }
            }
            ExprKind::New { class, args } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let (pos_arr, named_obj) = self.emit_call_args_pair(args, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                let st = self.boolean(self.file.strict_types)?;
                self.helper("newObj", &[cr, pos_arr, named_obj, sc, REG_CALLED, st])
            }
            ExprKind::AnonClass {
                is_readonly,
                extends,
                implements,
                args,
                members,
            } => {
                let seq = ANON_CLASS_SEQ.fetch_add(1, Ordering::Relaxed);
                let cname = format!("class@anonymous#{seq}");
                let spec = self.emit_class_spec(
                    "class",
                    &cname,
                    false,
                    false,
                    *is_readonly,
                    None,
                    extends.as_ref(),
                    &[],
                    implements,
                    &[],
                    members,
                    &[],
                    depth + 1,
                )?;
                let cls_meta = self.helper("defClass", &[spec])?;
                let (pos_arr, named_obj) = self.emit_call_args_pair(args, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                let st = self.boolean(self.file.strict_types)?;
                self.helper("newObj", &[cls_meta, pos_arr, named_obj, sc, REG_CALLED, st])
            }
            ExprKind::Clone(inner) => {
                let v = self.emit_expr(inner, depth + 1)?;
                self.helper("cloneObj", &[v])
            }
            ExprKind::InstanceOf { expr: inner, class } => {
                let v = self.emit_expr(inner, depth + 1)?;
                let cr = self.emit_class_ref(class, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("instanceOf", &[v, cr, sc, REG_CALLED])
            }
            ExprKind::Closure {
                is_static,
                by_ref,
                params,
                uses,
                return_type,
                body,
                ..
            } => {
                let (fid, is_gen, has_func_args, is_direct) = self.compile_php_function(
                    "{closure}",
                    params,
                    uses,
                    body,
                    self.class_name.clone(),
                    self.trait_name.clone(),
                    "{closure}".to_owned(),
                    &[],
                )?;
                let code = self.alloc()?;
                self.emit(Instr::MakeFunc {
                    dst: code,
                    func_id: fid,
                })?;
                let meta = self.new_js_obj()?;
                let pm = self.emit_params_meta(params)?;
                self.set_prop(meta, "params", pm)?;
                let br = self.boolean(*by_ref)?;
                self.set_prop(meta, "byRef", br)?;
                let ist = self.boolean(*is_static)?;
                self.set_prop(meta, "isStatic", ist)?;
                let ig = self.boolean(is_gen)?;
                self.set_prop(meta, "isGen", ig)?;
                if has_func_args {
                    let hfa = self.boolean(true)?;
                    self.set_prop(meta, "hasFuncArgs", hfa)?;
                }
                if is_direct {
                    let dr = self.boolean(true)?;
                    self.set_prop(meta, "direct", dr)?;
                }
                let st = self.boolean(self.file.strict_types)?;
                self.set_prop(meta, "strict", st)?;
                if let Some(rt) = return_type {
                    let ts = self.ns.format_type(rt);
                    let tr = self.string(&ts)?;
                    self.set_prop(meta, "retType", tr)?;
                }
                let mut cap_regs = Vec::with_capacity(uses.len());
                for u in uses {
                    if u.by_ref {
                        cap_regs.push(self.load_var_ref(&u.name)?);
                    } else {
                        let v = self.load_var(&u.name, false)?;
                        cap_regs.push(self.helper("share", &[v])?);
                    }
                }
                let uses_arr = self.js_array(&cap_regs)?;
                let sc = self.scope_cls_reg()?;
                self.helper("mkClosure", &[code, meta, uses_arr, REG_THIS, sc])
            }
            ExprKind::ArrowFn {
                is_static,
                by_ref,
                params,
                return_type,
                body,
                ..
            } => {
                let caps = arrow_captures(params, body);
                let uses: Vec<ClosureUse> = caps
                    .iter()
                    .map(|n| ClosureUse {
                        name: n.clone(),
                        by_ref: false,
                        span: expr.span,
                    })
                    .collect();
                let ret_stmt = Stmt {
                    kind: StmtKind::Return(Some((**body).clone())),
                    span: body.span,
                };
                let (fid, is_gen, has_func_args, is_direct) = self.compile_php_function(
                    "{closure}",
                    params,
                    &uses,
                    &[ret_stmt],
                    self.class_name.clone(),
                    self.trait_name.clone(),
                    "{closure}".to_owned(),
                    &[],
                )?;
                let code = self.alloc()?;
                self.emit(Instr::MakeFunc {
                    dst: code,
                    func_id: fid,
                })?;
                let meta = self.new_js_obj()?;
                let pm = self.emit_params_meta(params)?;
                self.set_prop(meta, "params", pm)?;
                let br = self.boolean(*by_ref)?;
                self.set_prop(meta, "byRef", br)?;
                let ist = self.boolean(*is_static)?;
                self.set_prop(meta, "isStatic", ist)?;
                let ig = self.boolean(is_gen)?;
                self.set_prop(meta, "isGen", ig)?;
                if has_func_args {
                    let hfa = self.boolean(true)?;
                    self.set_prop(meta, "hasFuncArgs", hfa)?;
                }
                if is_direct {
                    let dr = self.boolean(true)?;
                    self.set_prop(meta, "direct", dr)?;
                }
                let st = self.boolean(self.file.strict_types)?;
                self.set_prop(meta, "strict", st)?;
                if let Some(rt) = return_type {
                    let ts = self.ns.format_type(rt);
                    let tr = self.string(&ts)?;
                    self.set_prop(meta, "retType", tr)?;
                }
                let mut cap_regs = Vec::with_capacity(caps.len());
                for c in &caps {
                    let v = self.load_var(c, true)?;
                    cap_regs.push(self.helper("share", &[v])?);
                }
                let uses_arr = self.js_array(&cap_regs)?;
                let sc = self.scope_cls_reg()?;
                self.helper("mkClosure", &[code, meta, uses_arr, REG_THIS, sc])
            }
            ExprKind::Match { cond, arms } => {
                let subj = self.emit_expr(cond, depth + 1)?;
                let dst = self.alloc()?;
                let mut done_jumps = Vec::new();
                let mut default_arm: Option<&Expr> = None;
                for arm in arms {
                    let Some(conds) = &arm.conds else {
                        default_arm = Some(&arm.body);
                        continue;
                    };
                    let mut hit_jumps = Vec::new();
                    for ce in conds {
                        let cv = self.emit_expr(ce, depth + 1)?;
                        let eq = self.helper("seq", &[subj, cv])?;
                        hit_jumps.push(self.jump_if_true(eq)?);
                    }
                    let miss = self.jump()?;
                    let body_at = self.here();
                    for hj in hit_jumps {
                        self.patch(hj, body_at)?;
                    }
                    let bv = self.emit_expr(&arm.body, depth + 1)?;
                    self.emit(Instr::Move { dst, src: bv })?;
                    done_jumps.push(self.jump()?);
                    let after = self.here();
                    self.patch(miss, after)?;
                }
                if let Some(def) = default_arm {
                    let bv = self.emit_expr(def, depth + 1)?;
                    self.emit(Instr::Move { dst, src: bv })?;
                } else {
                    self.helper("matchErr", &[subj])?;
                }
                let end = self.here();
                for dj in done_jumps {
                    self.patch(dj, end)?;
                }
                Ok(dst)
            }
            ExprKind::Throw(inner) => {
                let v = self.emit_expr(inner, depth + 1)?;
                self.helper("throwVal", &[v])?;
                self.null()
            }
            ExprKind::Yield { key, value } => {
                let (hk, kr) = match key {
                    Some(k) => (self.boolean(true)?, self.emit_expr(k, depth + 1)?),
                    None => (self.boolean(false)?, self.null()?),
                };
                let vr = match value {
                    Some(v) => self.emit_expr(v, depth + 1)?,
                    None => self.null()?,
                };
                let pair = self.helper("yieldVal", &[hk, kr, vr])?;
                let dst = self.alloc()?;
                self.emit(Instr::Yield { dst, val: pair })?;
                Ok(dst)
            }
            ExprKind::YieldFrom(inner) => {
                let src = self.emit_expr(inner, depth + 1)?;
                let br = self.boolean(false)?;
                let it = self.helper("iterStart", &[src, br])?;
                let head = self.here();
                let step = self.helper("iterNext", &[it])?;
                let exit = self.jump_if_false(step)?;
                let hk = self.boolean(true)?;
                let kv = self.prop(it, "key")?;
                let vv = self.prop(it, "val")?;
                let pair = self.helper("yieldVal", &[hk, kv, vv])?;
                let sink_r = self.alloc()?;
                self.emit(Instr::Yield {
                    dst: sink_r,
                    val: pair,
                })?;
                let back = self.jump()?;
                self.patch(back, head)?;
                let end = self.here();
                self.patch(exit, end)?;
                self.null()
            }
            ExprKind::Isset(list) => {
                let dst = self.alloc()?;
                let mut fail_jumps = Vec::new();
                for item in list {
                    let ok = self.emit_isset_one(item, depth + 1)?;
                    fail_jumps.push(self.jump_if_false(ok)?);
                }
                let t = self.boolean(true)?;
                self.emit(Instr::Move { dst, src: t })?;
                let done = self.jump()?;
                let fail_at = self.here();
                for fj in fail_jumps {
                    self.patch(fj, fail_at)?;
                }
                let f = self.boolean(false)?;
                self.emit(Instr::Move { dst, src: f })?;
                let end = self.here();
                self.patch(done, end)?;
                Ok(dst)
            }
            ExprKind::Empty(inner) => {
                let v = self.emit_expr_quiet(inner, depth + 1)?;
                self.helper("empty", &[v])
            }
            ExprKind::Print(inner) => {
                let v = self.emit_expr(inner, depth + 1)?;
                self.helper("print", &[v])
            }
            ExprKind::Include { kind, expr: inner } => {
                let k = self.small_int(match kind {
                    IncludeKind::Include => 0,
                    IncludeKind::IncludeOnce => 1,
                    IncludeKind::Require => 2,
                    IncludeKind::RequireOnce => 3,
                })?;
                let path = self.emit_expr(inner, depth + 1)?;
                let cf = self.file.file.clone();
                let cd = self.file.dir.clone();
                let cfr = self.string(&cf)?;
                let cdr = self.string(&cd)?;
                let scope = self.scope_map_reg()?;
                self.helper("include", &[k, path, cfr, cdr, scope])
            }
            ExprKind::Exit(opt) => {
                let v = match opt {
                    Some(e) => self.emit_expr(e, depth + 1)?,
                    None => self.null()?,
                };
                self.helper("exit", &[v])
            }
            ExprKind::Eval(inner) => {
                let code = self.emit_expr(inner, depth + 1)?;
                let cd = self.file.dir.clone();
                let cdr = self.string(&cd)?;
                let scope = self.scope_map_reg()?;
                self.helper("evalCode", &[code, cdr, scope])
            }
        }
    }

    /// Quiet expression read (used by `isset`/`empty`/`??` to avoid warnings on missing keys/vars).
    fn emit_expr_quiet(&mut self, expr: &Expr, depth: usize) -> R<Reg> {
        match &expr.kind {
            ExprKind::Var(name) => self.load_var(name, true),
            ExprKind::Index {
                target,
                index: Some(idx),
            } => {
                let t = self.emit_expr_quiet(target, depth + 1)?;
                let k = self.emit_expr(idx, depth + 1)?;
                let q = self.boolean(true)?;
                self.helper("idxGet", &[t, k, q])
            }
            ExprKind::Prop {
                target,
                prop,
                nullsafe,
            } => {
                let obj = self.emit_expr_quiet(target, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let ns = self.boolean(*nullsafe)?;
                let sc = self.scope_cls_reg()?;
                let q = self.boolean(true)?;
                self.helper("propGet", &[obj, pn, ns, sc, q])
            }
            _ => self.emit_expr(expr, depth),
        }
    }

    fn emit_isset_one(&mut self, expr: &Expr, depth: usize) -> R<Reg> {
        match &expr.kind {
            ExprKind::Index {
                target,
                index: Some(idx),
            } => {
                let t = self.emit_expr_quiet(target, depth + 1)?;
                let k = self.emit_expr(idx, depth + 1)?;
                self.helper("idxIsset", &[t, k])
            }
            ExprKind::Prop { target, prop, .. } => {
                let obj = self.emit_expr_quiet(target, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("propIsset", &[obj, pn, sc])
            }
            _ => {
                let v = self.emit_expr_quiet(expr, depth + 1)?;
                let n = self.null()?;
                let dst = self.alloc()?;
                self.emit(Instr::Ne { dst, a: v, b: n })?;
                Ok(dst)
            }
        }
    }

    pub fn emit_prop_name(&mut self, prop: &PropName, depth: usize) -> R<Reg> {
        match prop {
            PropName::Ident(s) => self.string(s),
            PropName::Expr(e) => self.emit_expr(e, depth),
        }
    }

    fn emit_class_ref(&mut self, class: &ClassRef, depth: usize) -> R<Reg> {
        match class {
            ClassRef::Name(n) => {
                let resolved = self.ns.resolve_class(n);
                self.string(&resolved)
            }
            ClassRef::Expr(e) => self.emit_expr(e, depth),
        }
    }

    fn scope_map_reg(&mut self) -> R<Reg> {
        match self.r_scope {
            Some(r) => Ok(r),
            None => self.null(),
        }
    }

    fn emit_one_call_arg(&mut self, a: &Arg, by_val_only: bool, depth: usize) -> R<Reg> {
        self.emit_one_call_arg_ex(a, 0, by_val_only, 0, depth)
    }

    fn emit_one_call_arg_ex(
        &mut self,
        a: &Arg,
        idx: usize,
        by_val_only: bool,
        ref_mask: u32,
        depth: usize,
    ) -> R<Reg> {
        if ref_mask != 0 && idx < 32 && (ref_mask & (1u32 << idx)) != 0 {
            let is_lv = matches!(
                a.value.kind,
                ExprKind::Var(ref n) if n != "this"
            ) || matches!(
                a.value.kind,
                ExprKind::DynamicVar(_)
                    | ExprKind::Index { .. }
                    | ExprKind::Prop { .. }
                    | ExprKind::StaticProp { .. }
            );
            if is_lv {
                return self.emit_lvalue_ref(&a.value, depth);
            }
        }
        if !by_val_only {
            if let ExprKind::Var(vname) = &a.value.kind {
                if vname != "this" {
                    return self.load_var_ref(vname);
                }
            }
        }
        self.emit_expr(&a.value, depth)
    }

    fn emit_call_args_pair(&mut self, args: &[Arg], depth: usize) -> R<(Reg, Reg)> {
        self.emit_call_args_pair_mask(args, false, 0, depth)
    }

    fn emit_call_args_pair_ex(
        &mut self,
        args: &[Arg],
        by_val_only: bool,
        depth: usize,
    ) -> R<(Reg, Reg)> {
        self.emit_call_args_pair_mask(args, by_val_only, 0, depth)
    }

    /// Build `(posArgsArray, namedArgsObjOrNull)` for a call argument list.
    /// Passes live `PhpRef`s for lvalue arguments at `ref_mask` bit positions, and for bare `$var`
    /// positional arguments when `by_val_only` is false.
    fn emit_call_args_pair_mask(
        &mut self,
        args: &[Arg],
        by_val_only: bool,
        ref_mask: u32,
        depth: usize,
    ) -> R<(Reg, Reg)> {
        let has_spread = args.iter().any(|a| a.unpack);
        let has_named = args.iter().any(|a| a.name.is_some());

        if !has_spread && !has_named {
            let mut regs = Vec::with_capacity(args.len());
            for (i, a) in args.iter().enumerate() {
                regs.push(self.emit_one_call_arg_ex(a, i, by_val_only, ref_mask, depth)?);
            }
            let pos = self.js_array(&regs)?;
            let named = self.null()?;
            return Ok((pos, named));
        }

        let pos = self.js_array(&[])?;
        let named = self.new_js_obj()?;
        for a in args {
            if a.unpack {
                let v = self.emit_expr(&a.value, depth)?;
                self.helper("spreadCallArgs", &[pos, named, v])?;
            } else if let Some(k) = &a.name {
                let v = if let ExprKind::Var(vname) = &a.value.kind {
                    if vname != "this" {
                        self.load_var_ref(vname)?
                    } else {
                        self.emit_expr(&a.value, depth)?
                    }
                } else {
                    self.emit_expr(&a.value, depth)?
                };
                self.set_prop(named, k, v)?;
            } else {
                let v = if let ExprKind::Var(vname) = &a.value.kind {
                    if vname != "this" {
                        self.load_var_ref(vname)?
                    } else {
                        self.emit_expr(&a.value, depth)?
                    }
                } else {
                    self.emit_expr(&a.value, depth)?
                };
                let push_fn = self.prop(pos, "push")?;
                let base = self.block(1)?;
                self.emit(Instr::Move { dst: base, src: v })?;
                let tmp = self.alloc()?;
                self.emit(Instr::CallWithThis {
                    dst: tmp,
                    callee: push_fn,
                    this_v: pos,
                    arg_base: base,
                    argc: 1,
                    name: crate::bytecode::NO_NAME,
                })?;
            }
        }
        Ok((pos, named))
    }

    /// Obtain a live `PhpRef` for an lvalue (`&$x`, `&$a['k']`, `&$o->p`, `&Cls::$p`).
    pub fn emit_lvalue_ref(&mut self, expr: &Expr, depth: usize) -> R<Reg> {
        match &expr.kind {
            ExprKind::Var(name) => self.load_var_ref(name),
            ExprKind::DynamicVar(inner) => {
                let n = self.emit_expr(inner, depth + 1)?;
                let scope = self.scope_map_reg()?;
                self.helper("sref", &[scope, n])
            }
            ExprKind::Index { .. } => {
                let (root, keys) = peel_index_chain(expr);
                let root_ref = self.emit_lvalue_ref(root, depth + 1)?;
                let mut kregs = Vec::with_capacity(keys.len());
                for k in keys {
                    match k {
                        Some(ke) => kregs.push(self.emit_expr(ke, depth + 1)?),
                        None => kregs.push(self.undefined()?),
                    }
                }
                let karr = self.js_array(&kregs)?;
                self.helper("idxRefPath", &[root_ref, karr])
            }
            ExprKind::Prop { target, prop, .. } => {
                let obj = self.emit_expr(target, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("propRef", &[obj, pn, sc])
            }
            ExprKind::StaticProp { class, prop } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("staticRef", &[cr, pn, sc, REG_CALLED])
            }
            _ => {
                let v = self.emit_expr(expr, depth + 1)?;
                self.helper("ref", &[v])
            }
        }
    }

    /// Assign a value `val` to an lvalue target (marks `PhpArray` as shared unless `owned == true`).
    pub fn emit_assign_target(&mut self, target: &Expr, val: Reg, depth: usize) -> R<Reg> {
        self.emit_assign_target_ex(target, val, false, depth)
    }

    /// Assign an already-owned value (known scalar, fresh array literal, or CoW write-back)
    /// without calling `R.share()`, preserving O(1) in-place CoW mutations!
    pub fn emit_assign_target_owned(&mut self, target: &Expr, val: Reg, depth: usize) -> R<Reg> {
        self.emit_assign_target_ex(target, val, true, depth)
    }

    fn emit_assign_target_ex(
        &mut self,
        target: &Expr,
        val: Reg,
        owned: bool,
        depth: usize,
    ) -> R<Reg> {
        match &target.kind {
            ExprKind::Var(name) => {
                if owned {
                    self.store_var_owned(name, val)
                } else {
                    self.store_var(name, val)
                }
            }
            ExprKind::DynamicVar(inner) => {
                let n = self.emit_expr(inner, depth + 1)?;
                let scope = self.scope_map_reg()?;
                self.helper(if owned { "ssetOwned" } else { "sset" }, &[scope, n, val])
            }
            ExprKind::Index { .. } => {
                let (root, keys) = peel_index_chain(target);
                let root_val = self.emit_expr_quiet(root, depth + 1)?;
                // Fast 1-level `$a[$k] = $v` / `$a[] = $v`: avoids allocating `karr` and `idxSetPath`'s recursive closure!
                let updated = if keys.len() == 1 {
                    match keys[0] {
                        Some(ke) => {
                            let k = self.emit_expr(ke, depth + 1)?;
                            self.helper("idxSet", &[root_val, k, val])?
                        }
                        None => self.helper("idxPush", &[root_val, val])?,
                    }
                } else {
                    let mut kregs = Vec::with_capacity(keys.len());
                    for k in keys {
                        match k {
                            Some(ke) => kregs.push(self.emit_expr(ke, depth + 1)?),
                            None => kregs.push(self.undefined()?),
                        }
                    }
                    let karr = self.js_array(&kregs)?;
                    self.helper("idxSetPath", &[root_val, karr, val])?
                };
                // Write back using `owned = true` so `updated` keeps `shared == false` (fixes O(N^2) CoW cloning!).
                if matches!(
                    root.kind,
                    ExprKind::Var(_)
                        | ExprKind::DynamicVar(_)
                        | ExprKind::Prop { .. }
                        | ExprKind::StaticProp { .. }
                ) {
                    self.emit_assign_target_owned(root, updated, depth + 1)?;
                }
                Ok(val)
            }
            ExprKind::Prop { target: obj_e, prop, .. } => {
                // Fast inline `$this->prop = val` write via monomorphic `REG_THIS.fp.<prop>` (zero JS calls!).
                if let (ExprKind::Var(vname), PropName::Ident(pname)) = (&obj_e.kind, prop) {
                    if vname == "this" {
                        if let Some(&expected_ty) = self.class_fast_props.get(pname) {
                            let fp = self.prop(REG_THIS, "fp")?;
                            let j_slow1 = self.jump_if_false(fp)?;
                            let j_slow2 = match expected_ty {
                                Some(ty_code) => {
                                    let ok = self.type_of_is(val, ty_code, false)?;
                                    self.jump_if_false(ok)?
                                }
                                None => {
                                    let is_obj = self.type_of_is(val, TY_OBJECT, false)?;
                                    self.jump_if_true(is_obj)?
                                }
                            };
                            self.set_prop(fp, pname, val)?;
                            let j_done = self.jump()?;
                            let slow = self.here();
                            self.patch(j_slow1, slow)?;
                            self.patch(j_slow2, slow)?;
                            let pn = self.string(pname)?;
                            let sc = self.scope_cls_reg()?;
                            self.helper("propSet", &[REG_THIS, pn, val, sc])?;
                            let end = self.here();
                            self.patch(j_done, end)?;
                            return Ok(val);
                        }
                    }
                }
                let obj = self.emit_expr(obj_e, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("propSet", &[obj, pn, val, sc])
            }
            ExprKind::StaticProp { class, prop } => {
                let cr = self.emit_class_ref(class, depth + 1)?;
                let pn = self.emit_prop_name(prop, depth + 1)?;
                let sc = self.scope_cls_reg()?;
                self.helper("staticSet", &[cr, pn, val, sc, REG_CALLED])
            }
            ExprKind::Array(items) | ExprKind::List(items) => {
                let q = self.boolean(true)?;
                for (i, it) in items.iter().enumerate() {
                    if matches!(it.value.kind, ExprKind::Null) && it.key.is_none() {
                        continue; // empty slot `[, $b] = $arr`
                    }
                    let k = match &it.key {
                        Some(ke) => self.emit_expr(ke, depth + 1)?,
                        None => self.int64(i as i64)?,
                    };
                    let elem = self.helper("idxGet", &[val, k, q])?;
                    self.emit_assign_target(&it.value, elem, depth + 1)?;
                }
                Ok(val)
            }
            _ => Err(format!(
                "PHP: invalid assignment target at {}:{}",
                self.file.file,
                self.file.line_of(target.span.start)
            )),
        }
    }

    // ---- Inline fast bytecode helpers (pure base Instr, exact PHP 8.4 semantics) ----

    /// Evaluate `expr` as a JS boolean, skipping `R.truthy` whenever `expr` is statically boolean.
    pub fn emit_cond_bool(&mut self, expr: &Expr, depth: usize) -> R<Reg> {
        let v = self.emit_expr(expr, depth)?;
        if is_known_bool(expr) {
            Ok(v)
        } else {
            self.truthy(v)
        }
    }

    /// Fast inline `++$x` / `--$x` for unboxed `small_bigint`, falling back to `R.inc`/`R.dec` on overflow or non-int.
    fn emit_fast_inc_dec(&mut self, cur: Reg, is_inc: bool) -> R<Reg> {
        let dst = self.alloc()?;
        let is_bi = self.type_of_is(cur, TY_BIGINT, false)?;
        let j_slow1 = self.jump_if_false(is_bi)?;
        let bound = self.int64(if is_inc { SAFE_SMALL_INT } else { -SAFE_SMALL_INT })?;
        let in_range = self.alloc()?;
        self.emit(if is_inc {
            Instr::Lt { dst: in_range, a: cur, b: bound }
        } else {
            Instr::Gt { dst: in_range, a: cur, b: bound }
        })?;
        let j_slow2 = self.jump_if_false(in_range)?;
        self.emit(Instr::AddInt {
            dst,
            a: cur,
            imm: if is_inc { 1 } else { -1 },
            upd: true,
        })?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        self.patch(j_slow1, slow_at)?;
        self.patch(j_slow2, slow_at)?;
        let s = self.helper(if is_inc { "inc" } else { "dec" }, &[cur])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    /// Fast inline `===` / `!==`: when `lv` is not an object (`typeof lv !== "object"`), JS `Eq`/`Ne` is exact PHP!
    fn emit_fast_strict_eq(&mut self, lv: Reg, rv: Reg, is_eq: bool) -> R<Reg> {
        let dst = self.alloc()?;
        let not_obj = self.type_of_is(lv, TY_OBJECT, true)?;
        let j_slow = self.jump_if_false(not_obj)?;
        self.emit(if is_eq {
            Instr::Eq { dst, a: lv, b: rv }
        } else {
            Instr::Ne { dst, a: lv, b: rv }
        })?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        self.patch(j_slow, slow_at)?;
        let s = self.helper(if is_eq { "seq" } else { "sneq" }, &[lv, rv])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    /// Fast inline `<`, `<=`, `>`, `>=` when both operands are `BigInt` (zero overflow possible!).
    fn emit_fast_int_cmp(
        &mut self,
        op: BinOp,
        lv: Reg,
        l_int: bool,
        rv: Reg,
        r_int: bool,
    ) -> R<Reg> {
        let dst = self.alloc()?;
        let j1 = if l_int {
            None
        } else {
            let t = self.type_of_is(lv, TY_BIGINT, false)?;
            Some(self.jump_if_false(t)?)
        };
        let j2 = if r_int {
            None
        } else {
            let t = self.type_of_is(rv, TY_BIGINT, false)?;
            Some(self.jump_if_false(t)?)
        };
        self.emit(match op {
            BinOp::Lt => Instr::Lt { dst, a: lv, b: rv },
            BinOp::LtEq => Instr::Le { dst, a: lv, b: rv },
            BinOp::Gt => Instr::Gt { dst, a: lv, b: rv },
            BinOp::GtEq => Instr::Ge { dst, a: lv, b: rv },
            _ => unreachable!(),
        })?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        if let Some(j) = j1 {
            self.patch(j, slow_at)?;
        }
        if let Some(j) = j2 {
            self.patch(j, slow_at)?;
        }
        let h = match op {
            BinOp::Lt => "lt",
            BinOp::LtEq => "lte",
            BinOp::Gt => "gt",
            BinOp::GtEq => "gte",
            _ => unreachable!(),
        };
        let s = self.helper(h, &[lv, rv])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    /// Fast inline `lv + imm` / `lv - imm` (for small `imm` in `±1_000_000`) on unboxed `small_bigint`.
    fn emit_fast_add_imm(&mut self, lv: Reg, l_int: bool, imm: i32) -> R<Reg> {
        let dst = self.alloc()?;
        let j1 = if l_int {
            None
        } else {
            let t = self.type_of_is(lv, TY_BIGINT, false)?;
            Some(self.jump_if_false(t)?)
        };
        let bound = self.int64(if imm > 0 { SAFE_SMALL_INT } else { -SAFE_SMALL_INT })?;
        let ok = self.alloc()?;
        self.emit(if imm > 0 {
            Instr::Lt { dst: ok, a: lv, b: bound }
        } else {
            Instr::Gt { dst: ok, a: lv, b: bound }
        })?;
        let j2 = self.jump_if_false(ok)?;
        self.emit(Instr::AddInt {
            dst,
            a: lv,
            imm,
            upd: true,
        })?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        if let Some(j) = j1 {
            self.patch(j, slow_at)?;
        }
        self.patch(j2, slow_at)?;
        let rhs_r = self.int64((imm as i64).abs())?;
        let s = self.helper(if imm > 0 { "add" } else { "sub" }, &[lv, rhs_r])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    /// Fast inline `lv + rv` / `lv - rv` / `lv * rv` on two `BigInt`s with `±SAFE_SMALL_INT` unboxed bounds check.
    fn emit_fast_int_arith(
        &mut self,
        op: BinOp,
        lv: Reg,
        l_int: bool,
        rv: Reg,
        r_int: bool,
    ) -> R<Reg> {
        let dst = self.alloc()?;
        let j1 = if l_int {
            None
        } else {
            let t = self.type_of_is(lv, TY_BIGINT, false)?;
            Some(self.jump_if_false(t)?)
        };
        let j2 = if r_int {
            None
        } else {
            let t = self.type_of_is(rv, TY_BIGINT, false)?;
            Some(self.jump_if_false(t)?)
        };
        self.emit(match op {
            BinOp::Add => Instr::Add { dst, a: lv, b: rv },
            BinOp::Sub => Instr::Sub { dst, a: lv, b: rv },
            BinOp::Mul => Instr::Mul { dst, a: lv, b: rv },
            _ => unreachable!(),
        })?;
        // Check `dst` stayed in `[-SAFE_SMALL_INT, SAFE_SMALL_INT]` (strictly inside PHP i64 -> no float overflow!)
        let hi = self.int64(SAFE_SMALL_INT)?;
        let ok_hi = self.alloc()?;
        self.emit(Instr::Le { dst: ok_hi, a: dst, b: hi })?;
        let j3 = self.jump_if_false(ok_hi)?;
        let lo = self.int64(-SAFE_SMALL_INT)?;
        let ok_lo = self.alloc()?;
        self.emit(Instr::Ge { dst: ok_lo, a: dst, b: lo })?;
        let j4 = self.jump_if_false(ok_lo)?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        if let Some(j) = j1 {
            self.patch(j, slow_at)?;
        }
        if let Some(j) = j2 {
            self.patch(j, slow_at)?;
        }
        self.patch(j3, slow_at)?;
        self.patch(j4, slow_at)?;
        let h = match op {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            _ => unreachable!(),
        };
        let s = self.helper(h, &[lv, rv])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    /// Fast inline string concatenation: `string + (string | bigint)` uses JS `Instr::Add` (O(1) rope + fast BigInt decimal!).
    fn emit_fast_concat(
        &mut self,
        lv: Reg,
        l_str: bool,
        rv: Reg,
        r_str: bool,
        r_int: bool,
    ) -> R<Reg> {
        let dst = self.alloc()?;
        let jl = if l_str {
            None
        } else {
            let t = self.type_of_is(lv, TY_STRING, false)?;
            Some(self.jump_if_false(t)?)
        };
        let mut j_slow_r = None;
        if !r_str && !r_int {
            let is_s = self.type_of_is(rv, TY_STRING, false)?;
            let j_ok = self.jump_if_true(is_s)?;
            let is_bi = self.type_of_is(rv, TY_BIGINT, false)?;
            j_slow_r = Some(self.jump_if_false(is_bi)?);
            let fast_at = self.here();
            self.patch(j_ok, fast_at)?;
        }
        self.emit(Instr::Add { dst, a: lv, b: rv })?;
        let j_done = self.jump()?;
        let slow_at = self.here();
        if let Some(j) = jl {
            self.patch(j, slow_at)?;
        }
        if let Some(j) = j_slow_r {
            self.patch(j, slow_at)?;
        }
        let s = self.helper("concat", &[lv, rv])?;
        self.emit(Instr::Move { dst, src: s })?;
        let end = self.here();
        self.patch(j_done, end)?;
        Ok(dst)
    }

    pub fn emit_assign_ref_target(&mut self, target: &Expr, ref_reg: Reg, depth: usize) -> R<()> {
        match &target.kind {
            ExprKind::Var(name) => self.bind_var_ref(name, ref_reg),
            ExprKind::DynamicVar(inner) => {
                let n = self.emit_expr(inner, depth + 1)?;
                let scope = self.scope_map_reg()?;
                self.helper("sbind", &[scope, n, ref_reg])?;
                Ok(())
            }
            _ => {
                let v = self.helper("deref", &[ref_reg])?;
                self.emit_assign_target(target, v, depth)?;
                Ok(())
            }
        }
    }
}

#[allow(dead_code)]
fn _keep_scope() {
    let _ = REG_SCOPE;
}
