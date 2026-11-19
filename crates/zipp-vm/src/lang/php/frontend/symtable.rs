//! Scope & variable analysis for PHP functions, closures, arrow functions, and scripts.

use std::collections::BTreeSet;
use zipp_phpparse::ast::*;

#[derive(Debug, Default)]
pub(super) struct FnScopeInfo {
    /// True when the function needs a dynamic `Map<name, PhpRef>` scope
    /// (top-level scripts, `include`/`require`, `$$var`, `compact`, `extract`, `get_defined_vars`, `eval`).
    pub use_scope_map: bool,
    /// All local variable names referenced in this scope (excluding `$this` and superglobals).
    pub locals: BTreeSet<String>,
    /// Local variables that MUST live in `PhpRef` boxes (aliased via `=&`, `global`, `static`,
    /// `use(&$x)`, `foreach(... as &$v)`, by-ref param, or passed as bare `$v` argument to a call).
    pub ref_vars: BTreeSet<String>,
    /// True if the function body contains `yield` or `yield from` (outside nested closures/classes).
    pub is_generator: bool,
    /// True if the function body calls `func_get_args`, `func_num_args`, or `func_get_arg`.
    pub uses_func_args: bool,
}

const SUPERGLOBALS: &[&str] = &[
    "GLOBALS", "_SERVER", "_GET", "_POST", "_FILES", "_COOKIE", "_SESSION", "_REQUEST", "_ENV",
];

pub(super) fn is_superglobal(name: &str) -> bool {
    SUPERGLOBALS.contains(&name)
}

/// PHP standard builtins that NEVER take any parameter by reference.
/// Passing bare `$var` to these calls does not need a `PhpRef` box!
pub(super) fn is_known_by_val_builtin(name: &str) -> bool {
    let lo = name.to_ascii_lowercase();
    matches!(
        lo.as_str(),
        "strlen"
            | "count"
            | "sizeof"
            | "is_array"
            | "is_string"
            | "is_int"
            | "is_integer"
            | "is_long"
            | "is_float"
            | "is_double"
            | "is_real"
            | "is_bool"
            | "is_null"
            | "is_object"
            | "is_resource"
            | "is_scalar"
            | "is_numeric"
            | "is_callable"
            | "is_iterable"
            | "is_countable"
            | "gettype"
            | "get_debug_type"
            | "intval"
            | "floatval"
            | "doubleval"
            | "strval"
            | "boolval"
            | "strtolower"
            | "strtoupper"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "substr"
            | "strpos"
            | "stripos"
            | "strrpos"
            | "strripos"
            | "str_starts_with"
            | "str_ends_with"
            | "str_contains"
            | "str_repeat"
            | "str_pad"
            | "str_split"
            | "strrev"
            | "strcmp"
            | "strcasecmp"
            | "strncmp"
            | "strncasecmp"
            | "substr_count"
            | "substr_replace"
            | "strstr"
            | "stristr"
            | "strchr"
            | "strrchr"
            | "strtr"
            | "strpbrk"
            | "strspn"
            | "strcspn"
            | "lcfirst"
            | "ucfirst"
            | "ucwords"
            | "implode"
            | "join"
            | "explode"
            | "sprintf"
            | "vsprintf"
            | "printf"
            | "vprintf"
            | "number_format"
            | "chr"
            | "ord"
            | "bin2hex"
            | "hex2bin"
            | "base64_encode"
            | "base64_decode"
            | "urlencode"
            | "urldecode"
            | "rawurlencode"
            | "rawurldecode"
            | "htmlspecialchars"
            | "htmlentities"
            | "html_entity_decode"
            | "http_build_query"
            | "parse_url"
            | "nl2br"
            | "strip_tags"
            | "addslashes"
            | "stripslashes"
            | "json_encode"
            | "json_decode"
            | "json_last_error"
            | "json_last_error_msg"
            | "array_key_exists"
            | "key_exists"
            | "in_array"
            | "array_search"
            | "array_keys"
            | "array_values"
            | "array_map"
            | "array_filter"
            | "array_reduce"
            | "array_merge"
            | "array_merge_recursive"
            | "array_replace"
            | "array_replace_recursive"
            | "array_slice"
            | "array_column"
            | "array_unique"
            | "array_reverse"
            | "array_flip"
            | "array_combine"
            | "array_diff"
            | "array_diff_key"
            | "array_diff_assoc"
            | "array_intersect"
            | "array_intersect_key"
            | "array_intersect_assoc"
            | "array_is_list"
            | "array_key_first"
            | "array_key_last"
            | "array_sum"
            | "array_product"
            | "array_chunk"
            | "array_pad"
            | "array_fill"
            | "array_fill_keys"
            | "array_change_key_case"
            | "array_count_values"
            | "array_find"
            | "array_find_key"
            | "array_any"
            | "array_all"
            | "range"
            | "abs"
            | "min"
            | "max"
            | "floor"
            | "ceil"
            | "round"
            | "sqrt"
            | "pow"
            | "intdiv"
            | "fmod"
            | "dechex"
            | "hexdec"
            | "decbin"
            | "bindec"
            | "time"
            | "microtime"
            | "hrtime"
            | "date"
            | "gmdate"
            | "strtotime"
            | "md5"
            | "sha1"
            | "crc32"
            | "hash"
            | "hash_hmac"
            | "hash_equals"
            | "random_bytes"
            | "random_int"
            | "password_hash"
            | "password_verify"
            | "get_class"
            | "get_parent_class"
            | "get_object_vars"
            | "get_class_methods"
            | "class_exists"
            | "interface_exists"
            | "trait_exists"
            | "enum_exists"
            | "function_exists"
            | "method_exists"
            | "property_exists"
            | "is_a"
            | "is_subclass_of"
            | "class_uses"
            | "class_parents"
            | "class_implements"
            | "spl_object_id"
            | "spl_object_hash"
            | "defined"
            | "constant"
            | "define"
            | "file_exists"
            | "is_file"
            | "is_dir"
            | "is_readable"
            | "is_writable"
            | "file_get_contents"
            | "file_put_contents"
            | "dirname"
            | "basename"
            | "realpath"
            | "pathinfo"
            | "header"
            | "http_response_code"
            | "ini_get"
            | "ini_set"
            | "extension_loaded"
            | "version_compare"
            | "phpversion"
            | "php_sapi_name"
            | "preg_quote"
            | "preg_split"
            | "preg_grep"
            | "ctype_alpha"
            | "ctype_alnum"
            | "ctype_digit"
            | "ctype_xdigit"
            | "ctype_lower"
            | "ctype_upper"
            | "ctype_space"
            | "mb_strlen"
            | "mb_substr"
            | "mb_strtolower"
            | "mb_strtoupper"
            | "mb_strpos"
            | "mb_stripos"
            | "mb_strrpos"
            | "mb_split"
            | "mb_str_split"
            | "mb_check_encoding"
            | "filter_var"
            | "serialize"
            | "unserialize"
            | "var_dump"
            | "print_r"
            | "var_export"
    )
}

pub(super) fn analyse_body(
    stmts: &[Stmt],
    params: &[Param],
    uses: &[ClosureUse],
    force_scope_map: bool,
) -> FnScopeInfo {
    let mut info = FnScopeInfo {
        use_scope_map: force_scope_map,
        ..Default::default()
    };
    for p in params {
        if !is_superglobal(&p.name) {
            info.locals.insert(p.name.clone());
            if p.by_ref {
                info.ref_vars.insert(p.name.clone());
            }
        }
    }
    for u in uses {
        if !is_superglobal(&u.name) {
            info.locals.insert(u.name.clone());
            if u.by_ref {
                info.ref_vars.insert(u.name.clone());
            }
        }
    }
    scan_stmts(stmts, &mut info);
    info
}

/// Collect free variables captured automatically by a PHP 7.4+ arrow function `fn($x) => expr`.
pub(super) fn arrow_captures(params: &[Param], body: &Expr) -> Vec<String> {
    let mut bound: BTreeSet<String> = params.iter().map(|p| p.name.clone()).collect();
    bound.insert("this".to_owned());
    let mut used = BTreeSet::new();
    collect_expr_vars(body, &mut used);
    used.into_iter()
        .filter(|v| !bound.contains(v) && !is_superglobal(v))
        .collect()
}

fn collect_expr_vars(expr: &Expr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Var(n) => {
            out.insert(n.clone());
        }
        ExprKind::DynamicVar(e)
        | ExprKind::Unary { expr: e, .. }
        | ExprKind::Cast { expr: e, .. }
        | ExprKind::Clone(e)
        | ExprKind::Throw(e)
        | ExprKind::Empty(e)
        | ExprKind::Print(e)
        | ExprKind::Include { expr: e, .. }
        | ExprKind::Eval(e)
        | ExprKind::YieldFrom(e) => collect_expr_vars(e, out),
        ExprKind::Exit(Some(e)) => collect_expr_vars(e, out),
        ExprKind::InterpString(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    collect_expr_vars(e, out);
                }
            }
        }
        ExprKind::Array(items) | ExprKind::List(items) => {
            for it in items {
                if let Some(k) = &it.key {
                    collect_expr_vars(k, out);
                }
                collect_expr_vars(&it.value, out);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_expr_vars(left, out);
            collect_expr_vars(right, out);
        }
        ExprKind::Assign { target, value, .. } => {
            collect_expr_vars(target, out);
            collect_expr_vars(value, out);
        }
        ExprKind::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_expr_vars(cond, out);
            if let Some(t) = then_expr {
                collect_expr_vars(t, out);
            }
            collect_expr_vars(else_expr, out);
        }
        ExprKind::Index { target, index } => {
            collect_expr_vars(target, out);
            if let Some(i) = index {
                collect_expr_vars(i, out);
            }
        }
        ExprKind::Prop { target, prop, .. } => {
            collect_expr_vars(target, out);
            if let PropName::Expr(e) = prop {
                collect_expr_vars(e, out);
            }
        }
        ExprKind::StaticProp { class, prop } => {
            if let ClassRef::Expr(e) = class {
                collect_expr_vars(e, out);
            }
            if let PropName::Expr(e) = prop {
                collect_expr_vars(e, out);
            }
        }
        ExprKind::ClassConst { class, name } => {
            if let ClassRef::Expr(e) = class {
                collect_expr_vars(e, out);
            }
            if let PropName::Expr(e) = name {
                collect_expr_vars(e, out);
            }
        }
        ExprKind::InstanceOf { expr, class } => {
            collect_expr_vars(expr, out);
            if let ClassRef::Expr(e) = class {
                collect_expr_vars(e, out);
            }
        }
        ExprKind::Call { callee, args } => {
            if let Callee::Expr(e) = callee {
                collect_expr_vars(e, out);
            }
            if let CallArgs::Normal(list) = args {
                for a in list {
                    collect_expr_vars(&a.value, out);
                }
            }
        }
        ExprKind::MethodCall {
            target,
            method,
            args,
            ..
        } => {
            collect_expr_vars(target, out);
            if let PropName::Expr(e) = method {
                collect_expr_vars(e, out);
            }
            if let CallArgs::Normal(list) = args {
                for a in list {
                    collect_expr_vars(&a.value, out);
                }
            }
        }
        ExprKind::StaticCall {
            class,
            method,
            args,
        } => {
            if let ClassRef::Expr(e) = class {
                collect_expr_vars(e, out);
            }
            if let PropName::Expr(e) = method {
                collect_expr_vars(e, out);
            }
            if let CallArgs::Normal(list) = args {
                for a in list {
                    collect_expr_vars(&a.value, out);
                }
            }
        }
        ExprKind::New { class, args } => {
            if let ClassRef::Expr(e) = class {
                collect_expr_vars(e, out);
            }
            for a in args {
                collect_expr_vars(&a.value, out);
            }
        }
        ExprKind::AnonClass { args, .. } => {
            for a in args {
                collect_expr_vars(&a.value, out);
            }
        }
        ExprKind::Closure { uses, .. } => {
            for u in uses {
                out.insert(u.name.clone());
            }
        }
        ExprKind::ArrowFn { params, body, .. } => {
            let nested = arrow_captures(params, body);
            for v in nested {
                out.insert(v);
            }
        }
        ExprKind::Match { cond, arms } => {
            collect_expr_vars(cond, out);
            for arm in arms {
                if let Some(cs) = &arm.conds {
                    for c in cs {
                        collect_expr_vars(c, out);
                    }
                }
                collect_expr_vars(&arm.body, out);
            }
        }
        ExprKind::Yield { key, value } => {
            if let Some(k) = key {
                collect_expr_vars(k, out);
            }
            if let Some(v) = value {
                collect_expr_vars(v, out);
            }
        }
        ExprKind::Isset(list) => {
            for e in list {
                collect_expr_vars(e, out);
            }
        }
        _ => {}
    }
}

fn scan_stmts(stmts: &[Stmt], info: &mut FnScopeInfo) {
    for s in stmts {
        scan_stmt(s, info);
    }
}

fn mark_var(name: &str, as_ref: bool, info: &mut FnScopeInfo) {
    if name == "this" || is_superglobal(name) {
        return;
    }
    info.locals.insert(name.to_owned());
    if as_ref {
        info.ref_vars.insert(name.to_owned());
    }
}

fn mark_lvalue_root_ref(expr: &Expr, info: &mut FnScopeInfo) {
    match &expr.kind {
        ExprKind::Var(n) => mark_var(n, true, info),
        ExprKind::Index { target, .. } => mark_lvalue_root_ref(target, info),
        _ => {}
    }
}

fn scan_stmt(stmt: &Stmt, info: &mut FnScopeInfo) {
    match &stmt.kind {
        StmtKind::InlineHtml(_) | StmtKind::Nop | StmtKind::Use(_) | StmtKind::Label(_) | StmtKind::Goto(_) => {}
        StmtKind::Expr(e) => scan_expr(e, info),
        StmtKind::Block(b) => scan_stmts(b, info),
        StmtKind::Echo(list) => {
            for e in list {
                scan_expr(e, info);
            }
        }
        StmtKind::Return(e) | StmtKind::Break(e) | StmtKind::Continue(e) => {
            if let Some(e) = e {
                scan_expr(e, info);
            }
        }
        StmtKind::Global(vars) => {
            for v in vars {
                mark_var(v, true, info);
            }
        }
        StmtKind::StaticVar(items) => {
            for (v, init) in items {
                mark_var(v, true, info);
                if let Some(e) = init {
                    scan_expr(e, info);
                }
            }
        }
        StmtKind::Unset(targets) => {
            for t in targets {
                scan_expr(t, info);
            }
        }
        StmtKind::If {
            cond,
            then_body,
            elseifs,
            else_body,
        } => {
            scan_expr(cond, info);
            scan_stmts(then_body, info);
            for (c, b) in elseifs {
                scan_expr(c, info);
                scan_stmts(b, info);
            }
            if let Some(eb) = else_body {
                scan_stmts(eb, info);
            }
        }
        StmtKind::While { cond, body } | StmtKind::DoWhile { cond, body } => {
            scan_expr(cond, info);
            scan_stmts(body, info);
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            for e in init.iter().chain(cond.iter()).chain(step.iter()) {
                scan_expr(e, info);
            }
            scan_stmts(body, info);
        }
        StmtKind::ForEach {
            expr,
            key,
            by_ref,
            value,
            body,
        } => {
            if *by_ref {
                mark_lvalue_root_ref(expr, info);
                mark_lvalue_root_ref(value, info);
            }
            scan_expr(expr, info);
            if let Some(k) = key {
                scan_expr(k, info);
            }
            scan_expr(value, info);
            scan_stmts(body, info);
        }
        StmtKind::Switch { cond, cases } => {
            scan_expr(cond, info);
            for c in cases {
                if let Some(e) = &c.cond {
                    scan_expr(e, info);
                }
                scan_stmts(&c.body, info);
            }
        }
        StmtKind::Try {
            body,
            catches,
            finally,
        } => {
            scan_stmts(body, info);
            for c in catches {
                if let Some(v) = &c.var {
                    mark_var(v, false, info);
                }
                scan_stmts(&c.body, info);
            }
            if let Some(f) = finally {
                scan_stmts(f, info);
            }
        }
        StmtKind::Namespace { body: Some(b), .. } | StmtKind::Declare { body: Some(b), .. } => {
            scan_stmts(b, info);
        }
        StmtKind::Const(items) => {
            for (_, e) in items {
                scan_expr(e, info);
            }
        }
        // Nested function / class / interface / trait / enum have their own scopes!
        StmtKind::Function(_)
        | StmtKind::Class(_)
        | StmtKind::Interface(_)
        | StmtKind::Trait(_)
        | StmtKind::Enum(_)
        | StmtKind::Namespace { body: None, .. }
        | StmtKind::Declare { body: None, .. } => {}
    }
}

/// Bitmask of 0-based positional argument indices taken by-reference by standard PHP builtins.
/// Returns `0` for non-builtin or unknown functions.
pub fn known_ref_arg_mask(name: &str) -> u32 {
    match name.trim_start_matches('\\').to_ascii_lowercase().as_str() {
        "array_push" | "array_pop" | "array_shift" | "array_unshift"
        | "array_splice" | "array_walk" | "array_walk_recursive"
        | "sort" | "rsort" | "asort" | "arsort" | "ksort" | "krsort"
        | "usort" | "uasort" | "uksort" | "natsort" | "natcasesort"
        | "shuffle" | "reset" | "end" | "next" | "prev" | "current" | "key"
        | "extract" => 1,
        "parse_str" => 2,
        "preg_match" | "preg_match_all" => 4,
        "str_replace" | "str_ireplace" | "preg_replace_callback_array" => 8,
        "preg_replace" | "preg_replace_callback" | "preg_filter" => 16,
        _ => 0,
    }
}

fn scan_call_args(args: &CallArgs, info: &mut FnScopeInfo, allow_by_ref: bool) {
    scan_call_args_ex(args, info, allow_by_ref, 0);
}

fn scan_call_args_ex(args: &CallArgs, info: &mut FnScopeInfo, allow_by_ref: bool, ref_mask: u32) {
    if let CallArgs::Normal(list) = args {
        for (i, a) in list.iter().enumerate() {
            if ref_mask != 0 {
                if i < 32 && (ref_mask & (1u32 << i)) != 0 {
                    mark_lvalue_root_ref(&a.value, info);
                }
            } else if allow_by_ref {
                // Any bare variable passed to an unknown call might be taken by-reference by the callee
                // (e.g. `swap($a, $b)`).
                if let ExprKind::Var(name) = &a.value.kind {
                    mark_var(name, true, info);
                }
            }
            scan_expr(&a.value, info);
        }
    }
}

fn scan_expr(expr: &Expr, info: &mut FnScopeInfo) {
    match &expr.kind {
        ExprKind::Var(name) => mark_var(name, false, info),
        ExprKind::DynamicVar(e) => {
            info.use_scope_map = true;
            scan_expr(e, info);
        }
        ExprKind::Include { expr: e, .. } | ExprKind::Eval(e) => {
            info.use_scope_map = true;
            scan_expr(e, info);
        }
        ExprKind::Yield { key, value } => {
            info.is_generator = true;
            if let Some(k) = key {
                scan_expr(k, info);
            }
            if let Some(v) = value {
                scan_expr(v, info);
            }
        }
        ExprKind::YieldFrom(e) => {
            info.is_generator = true;
            scan_expr(e, info);
        }
        ExprKind::Assign { op, target, value } => {
            if *op == AssignOp::AssignRef {
                mark_lvalue_root_ref(target, info);
                mark_lvalue_root_ref(value, info);
            }
            scan_expr(target, info);
            scan_expr(value, info);
        }
        ExprKind::Unary { expr: e, .. }
        | ExprKind::Cast { expr: e, .. }
        | ExprKind::Clone(e)
        | ExprKind::Throw(e)
        | ExprKind::Empty(e)
        | ExprKind::Print(e) => scan_expr(e, info),
        ExprKind::Exit(Some(e)) => scan_expr(e, info),
        ExprKind::InterpString(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    scan_expr(e, info);
                }
            }
        }
        ExprKind::Array(items) | ExprKind::List(items) => {
            for it in items {
                if it.by_ref {
                    mark_lvalue_root_ref(&it.value, info);
                }
                if let Some(k) = &it.key {
                    scan_expr(k, info);
                }
                scan_expr(&it.value, info);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            scan_expr(left, info);
            scan_expr(right, info);
        }
        ExprKind::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            scan_expr(cond, info);
            if let Some(t) = then_expr {
                scan_expr(t, info);
            }
            scan_expr(else_expr, info);
        }
        ExprKind::Index { target, index } => {
            scan_expr(target, info);
            if let Some(i) = index {
                scan_expr(i, info);
            }
        }
        ExprKind::Prop { target, prop, .. } => {
            scan_expr(target, info);
            if let PropName::Expr(e) = prop {
                scan_expr(e, info);
            }
        }
        ExprKind::StaticProp { class, prop } => {
            if let ClassRef::Expr(e) = class {
                scan_expr(e, info);
            }
            if let PropName::Expr(e) = prop {
                scan_expr(e, info);
            }
        }
        ExprKind::ClassConst { class, name } => {
            if let ClassRef::Expr(e) = class {
                scan_expr(e, info);
            }
            if let PropName::Expr(e) = name {
                scan_expr(e, info);
            }
        }
        ExprKind::InstanceOf { expr, class } => {
            scan_expr(expr, info);
            if let ClassRef::Expr(e) = class {
                scan_expr(e, info);
            }
        }
        ExprKind::Call { callee, args } => {
            let mut allow_by_ref = true;
            let mut ref_mask = 0u32;
            if let Callee::Name(n) = callee {
                if n.parts.len() == 1 {
                    let lo = n.parts[0].to_ascii_lowercase();
                    if matches!(lo.as_str(), "compact" | "extract" | "get_defined_vars") {
                        info.use_scope_map = true;
                    } else if matches!(lo.as_str(), "func_get_args" | "func_num_args" | "func_get_arg") {
                        info.uses_func_args = true;
                    }
                    ref_mask = known_ref_arg_mask(&lo);
                    if ref_mask != 0 || is_known_by_val_builtin(&lo) {
                        allow_by_ref = false;
                    }
                }
            } else if let Callee::Expr(e) = callee {
                scan_expr(e, info);
            }
            scan_call_args_ex(args, info, allow_by_ref, ref_mask);
        }
        ExprKind::MethodCall {
            target,
            method,
            args,
            ..
        } => {
            scan_expr(target, info);
            if let PropName::Expr(e) = method {
                scan_expr(e, info);
            }
            scan_call_args(args, info, true);
        }
        ExprKind::StaticCall {
            class,
            method,
            args,
        } => {
            if let ClassRef::Expr(e) = class {
                scan_expr(e, info);
            }
            if let PropName::Expr(e) = method {
                scan_expr(e, info);
            }
            scan_call_args(args, info, true);
        }
        ExprKind::New { class, args } => {
            if let ClassRef::Expr(e) = class {
                scan_expr(e, info);
            }
            for a in args {
                if let ExprKind::Var(v) = &a.value.kind {
                    mark_var(v, true, info);
                }
                scan_expr(&a.value, info);
            }
        }
        ExprKind::AnonClass { args, .. } => {
            for a in args {
                scan_expr(&a.value, info);
            }
        }
        ExprKind::Closure { uses, .. } => {
            for u in uses {
                mark_var(&u.name, u.by_ref, info);
            }
        }
        ExprKind::ArrowFn { params, body, .. } => {
            for v in arrow_captures(params, body) {
                mark_var(&v, false, info);
            }
        }
        ExprKind::Match { cond, arms } => {
            scan_expr(cond, info);
            for arm in arms {
                if let Some(cs) = &arm.conds {
                    for c in cs {
                        scan_expr(c, info);
                    }
                }
                scan_expr(&arm.body, info);
            }
        }
        ExprKind::Isset(list) => {
            for e in list {
                scan_expr(e, info);
            }
        }
        _ => {}
    }
}
