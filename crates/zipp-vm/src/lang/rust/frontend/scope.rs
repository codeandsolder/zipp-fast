//! Module tree resolution, symbol table, `use` alias expansion, static numeric type
//! inference (for truncating integer division & direct JIT fast paths), and
//! per-function closure/mutable-reference capture analysis.

use std::collections::{HashMap, HashSet};
use zipp_rsparse::ast::*;
use zipp_rsparse::token::{FloatSuffix, IntSuffix};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumKind {
    Int,
    Float,
    Unknown,
}

impl NumKind {
    pub fn from_type(ty: &Type) -> Self {
        match &ty.kind {
            TypeKind::Path(p) => match p.last_ident() {
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
                | "u128" | "usize" => NumKind::Int,
                "f32" | "f64" => NumKind::Float,
                _ => NumKind::Unknown,
            },
            TypeKind::Ref { inner, .. } => Self::from_type(inner),
            _ => NumKind::Unknown,
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, NumKind::Int | NumKind::Float)
    }
}

pub(super) fn type_base_name(ty: &Type) -> String {
    match &ty.kind {
        TypeKind::Path(p) => p.last_ident().to_owned(),
        TypeKind::Ref { inner, .. } | TypeKind::Ptr { inner, .. } => type_base_name(inner),
        TypeKind::Tuple(v) if v.is_empty() => "()".to_owned(),
        TypeKind::Tuple(_) => "tuple".to_owned(),
        TypeKind::Array(_, _) | TypeKind::Slice(_) => "Vec".to_owned(),
        _ => String::new(),
    }
}

/// Full recursive type descriptor (e.g. `Vec<User>`, `Option<Vec<Comment>>`, `HashMap<String, Item>`)
/// used by `serde_json` to deserialize nested structs and enums accurately.
pub(super) fn type_desc(ty: &Type) -> String {
    match &ty.kind {
        TypeKind::Path(p) => {
            let Some(last) = p.segments.last() else {
                return String::new();
            };
            if last.type_args.is_empty() {
                last.ident.clone()
            } else {
                let inner: Vec<String> = last.type_args.iter().map(type_desc).collect();
                format!("{}<{}>", last.ident, inner.join(", "))
            }
        }
        TypeKind::Ref { inner, .. } | TypeKind::Ptr { inner, .. } => type_desc(inner),
        TypeKind::Tuple(v) if v.is_empty() => "()".to_owned(),
        TypeKind::Tuple(v) => {
            let inner: Vec<String> = v.iter().map(type_desc).collect();
            format!("({})", inner.join(", "))
        }
        TypeKind::Array(elem, _) | TypeKind::Slice(elem) => format!("Vec<{}>", type_desc(elem)),
        _ => String::new(),
    }
}

/// Full path string for a type (e.g. `sqlx::Error`, `anyhow::Error`, `std::io::Error`),
/// used by `thiserror` `#[from]` auto-conversion registration.
pub(super) fn type_path_str(ty: &Type) -> String {
    match &ty.kind {
        TypeKind::Path(p) => p
            .segments
            .iter()
            .map(|s| s.ident.as_str())
            .collect::<Vec<_>>()
            .join("::"),
        TypeKind::Ref { inner, .. } | TypeKind::Ptr { inner, .. } => type_path_str(inner),
        _ => type_base_name(ty),
    }
}

/// Extract the error type name `E` from `-> Result<T, E>` or `-> anyhow::Result<T>`.
pub(super) fn extract_result_err_type(ret_ty: Option<&Type>) -> Option<String> {
    let ty = ret_ty?;
    let TypeKind::Path(p) = &ty.kind else {
        return None;
    };
    let last = p.segments.last()?;
    if last.ident == "Result" {
        if last.type_args.len() >= 2 {
            let name = type_base_name(&last.type_args[1]);
            if !name.is_empty() && name != "_" {
                return Some(name);
            }
        } else if p.segments.first().map(|s| s.ident.as_str()) == Some("anyhow") {
            return Some("anyhow::Error".to_owned());
        }
    }
    None
}

/// Extract the Ok / return type descriptor `T` from `-> Result<T, E>` or `-> T`,
/// used to propagate target types into trailing `sqlx::query_as` / `serde_json::from_str` returns.
pub(super) fn extract_return_ok_desc(ret_ty: Option<&Type>) -> Option<String> {
    let ty = ret_ty?;
    if let TypeKind::Path(p) = &ty.kind {
        if let Some(last) = p.segments.last() {
            if last.ident == "Result" && !last.type_args.is_empty() {
                let d = type_desc(&last.type_args[0]);
                return (!d.is_empty()).then_some(d);
            }
        }
    }
    let d = type_desc(ty);
    (!d.is_empty()).then_some(d)
}

/// Inspect a function's parameter list for Axum extractors (`State`, `Path`, `Query`, `Json`,
/// `Extension`, `ConnectInfo`, `HeaderMap`, `Request`, `Next`, `WebSocketUpgrade`, etc.).
/// Returns `Some(vec![(extractor_kind, inner_type_desc), ...])` if the function looks like
/// an Axum handler or middleware.
pub(super) fn extract_axum_params(
    syms: &CrateSymbols,
    mod_prefix: &str,
    params: &[FnParam],
) -> Option<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(params.len());
    let mut has_axum = false;
    for p in params {
        if p.is_self {
            return None;
        }
        let Some(ref ty) = p.ty else {
            return None;
        };
        let (kind, inner) = classify_axum_param(syms, mod_prefix, ty);
        // Note: a bare `Request` parameter without any Axum-specific extractor
        // (`State`, `Path`, `Query`, `Json`, `Next`, ...) may be a `zipp_http`
        // `(req: Request, state: Arc<...>)` handler, so only Axum-specific
        // extractors opt a function into Axum extractor dispatch.
        if matches!(
            kind.as_str(),
            "State"
                | "Path"
                | "Query"
                | "Json"
                | "Extension"
                | "ConnectInfo"
                | "HeaderMap"
                | "Next"
                | "WebSocketUpgrade"
                | "OriginalUri"
                | "Uri"
                | "Method"
                | "RawQuery"
        ) || kind.starts_with("Option:")
        {
            has_axum = true;
        }
        out.push((kind, inner));
    }
    if has_axum {
        Some(out)
    } else {
        None
    }
}

fn classify_axum_param(syms: &CrateSymbols, mod_prefix: &str, ty: &Type) -> (String, String) {
    let TypeKind::Path(p) = &ty.kind else {
        return ("Unknown".to_owned(), syms.type_desc_resolved(mod_prefix, ty));
    };
    let Some(last) = p.segments.last() else {
        return ("Unknown".to_owned(), String::new());
    };
    let inner = last
        .type_args
        .first()
        .map(|t| syms.type_desc_resolved(mod_prefix, t))
        .unwrap_or_default();
    match last.ident.as_str() {
        "State" | "Path" | "Query" | "Json" | "Extension" | "ConnectInfo" | "TypedHeader" => {
            (last.ident.clone(), inner)
        }
        "HeaderMap" | "Request" | "Next" | "WebSocketUpgrade" | "OriginalUri" | "Uri"
        | "Method" | "RawQuery" => (last.ident.clone(), inner),
        "Option" => {
            if let Some(first_arg) = last.type_args.first() {
                let (ik, ii) = classify_axum_param(syms, mod_prefix, first_arg);
                if ik != "Unknown" {
                    return (format!("Option:{ik}"), ii);
                }
            }
            ("Unknown".to_owned(), syms.type_desc_resolved(mod_prefix, ty))
        }
        "String" | "Bytes" => ("Body".to_owned(), last.ident.clone()),
        _ => ("Unknown".to_owned(), syms.type_desc_resolved(mod_prefix, ty)),
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct SerdeFieldMeta {
    pub rename: Option<String>,
    pub alias: Option<String>,
    pub skip: bool,
    pub skip_serializing: bool,
    pub skip_deserializing: bool,
    pub skip_serializing_if: Option<String>,
    pub flatten: bool,
    pub has_default: bool,
    pub default_fn: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SerdeContainerMeta {
    pub rename_all: Option<String>,
}

pub(super) fn extract_serde_container(attrs: &[Attribute]) -> SerdeContainerMeta {
    let mut meta = SerdeContainerMeta::default();
    for a in attrs {
        if a.path.last().map(|s| s.as_str()) == Some("serde") {
            if let AttrArgs::Delimited(toks) = &a.args {
                let mut i = 0;
                while i < toks.len() {
                    if let zipp_rsparse::token::TokenKind::Ident(k) = &toks[i].kind {
                        if k == "rename_all"
                            && matches!(
                                toks.get(i + 1).map(|t| &t.kind),
                                Some(zipp_rsparse::token::TokenKind::Eq)
                            )
                        {
                            if let Some(zipp_rsparse::token::TokenKind::Str(v)) =
                                toks.get(i + 2).map(|t| &t.kind)
                            {
                                meta.rename_all = Some(v.clone());
                                i += 3;
                                continue;
                            }
                        }
                    }
                    i += 1;
                }
            }
        }
    }
    meta
}

pub(super) fn extract_serde_field(attrs: &[Attribute]) -> SerdeFieldMeta {
    let mut meta = SerdeFieldMeta::default();
    for a in attrs {
        if a.path.last().map(|s| s.as_str()) == Some("serde") {
            if let AttrArgs::Delimited(toks) = &a.args {
                let mut i = 0;
                while i < toks.len() {
                    if let zipp_rsparse::token::TokenKind::Ident(k) = &toks[i].kind {
                        match k.as_str() {
                            "skip" => meta.skip = true,
                            "skip_serializing" => meta.skip_serializing = true,
                            "skip_deserializing" => meta.skip_deserializing = true,
                            "flatten" => meta.flatten = true,
                            "default" => {
                                meta.has_default = true;
                                if matches!(
                                    toks.get(i + 1).map(|t| &t.kind),
                                    Some(zipp_rsparse::token::TokenKind::Eq)
                                ) {
                                    if let Some(zipp_rsparse::token::TokenKind::Str(v)) =
                                        toks.get(i + 2).map(|t| &t.kind)
                                    {
                                        meta.default_fn = Some(v.clone());
                                        i += 3;
                                        continue;
                                    }
                                }
                            }
                            "rename" | "alias" | "skip_serializing_if" => {
                                if matches!(
                                    toks.get(i + 1).map(|t| &t.kind),
                                    Some(zipp_rsparse::token::TokenKind::Eq)
                                ) {
                                    if let Some(zipp_rsparse::token::TokenKind::Str(v)) =
                                        toks.get(i + 2).map(|t| &t.kind)
                                    {
                                        match k.as_str() {
                                            "rename" => meta.rename = Some(v.clone()),
                                            "alias" => meta.alias = Some(v.clone()),
                                            "skip_serializing_if" => {
                                                meta.skip_serializing_if = Some(v.clone())
                                            }
                                            _ => {}
                                        }
                                        i += 3;
                                        continue;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    i += 1;
                }
            }
        }
    }
    meta
}

/// Evaluate `#[cfg(...)]` attributes on an item or statement (supporting `test`,
/// `windows`, `unix`, `target_os`, `target_family`, `target_arch`, `debug_assertions`,
/// and nested `not(...)`, `all(...)`, `any(...)`).
pub(super) fn item_cfg_enabled(attrs: &[Attribute], is_test_mode: bool) -> bool {
    for a in attrs {
        if a.path.last().map(|s| s.as_str()) == Some("cfg") {
            if let AttrArgs::Delimited(toks) = &a.args {
                if !eval_cfg_tokens(toks, is_test_mode) {
                    return false;
                }
            }
        }
    }
    true
}

fn eval_cfg_tokens(toks: &[zipp_rsparse::token::Token], is_test_mode: bool) -> bool {
    use zipp_rsparse::token::TokenKind;
    if toks.is_empty() {
        return true;
    }
    // Single ident: `test`, `windows`, `unix`, `debug_assertions`
    if toks.len() == 1 {
        if let TokenKind::Ident(id) = &toks[0].kind {
            return match id.as_str() {
                "test" => is_test_mode,
                "windows" => cfg!(windows),
                "unix" => cfg!(unix),
                "debug_assertions" => false,
                _ => true,
            };
        }
    }
    // Key-value: `target_os = "linux"`, `target_family = "unix"`, `target_arch = "x86_64"`
    if toks.len() == 3 && matches!(toks[1].kind, TokenKind::Eq) {
        if let (TokenKind::Ident(k), TokenKind::Str(v)) = (&toks[0].kind, &toks[2].kind) {
            return match k.as_str() {
                "target_os" => match v.as_str() {
                    "windows" => cfg!(target_os = "windows"),
                    "linux" => cfg!(target_os = "linux"),
                    "macos" => cfg!(target_os = "macos"),
                    "freebsd" => cfg!(target_os = "freebsd"),
                    _ => false,
                },
                "target_family" => match v.as_str() {
                    "windows" => cfg!(target_family = "windows"),
                    "unix" => cfg!(target_family = "unix"),
                    "wasm" => cfg!(target_family = "wasm"),
                    _ => false,
                },
                "target_arch" => match v.as_str() {
                    "x86_64" => cfg!(target_arch = "x86_64"),
                    "aarch64" => cfg!(target_arch = "aarch64"),
                    "wasm32" => cfg!(target_arch = "wasm32"),
                    _ => false,
                },
                _ => true,
            };
        }
    }
    // Combinators: `not(...)`, `all(...)`, `any(...)`
    if toks.len() >= 3
        && matches!(toks[1].kind, TokenKind::LParen)
        && matches!(toks.last().map(|t| &t.kind), Some(TokenKind::RParen))
    {
        if let TokenKind::Ident(op) = &toks[0].kind {
            let inner = &toks[2..toks.len() - 1];
            match op.as_str() {
                "not" => return !eval_cfg_tokens(inner, is_test_mode),
                "all" => {
                    let parts = super::macros::split_top_level_commas(inner);
                    return parts.iter().all(|p| eval_cfg_tokens(p, is_test_mode));
                }
                "any" => {
                    let parts = super::macros::split_top_level_commas(inner);
                    return parts.iter().any(|p| eval_cfg_tokens(p, is_test_mode));
                }
                _ => {}
            }
        }
    }
    true
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct StructInfo {
    pub name: String,
    pub is_tuple: bool,
    pub is_unit: bool,
    pub fields: Vec<(String, String)>, // (field_name_or_idx, type_base_name)
    pub derives: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct EnumInfo {
    pub name: String,
    pub variants: HashMap<String, VariantKind>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VariantKind {
    Unit,
    Tuple,
    Struct,
}

/// Crate-wide resolved symbol metadata.
#[derive(Default)]
pub(super) struct CrateSymbols {
    /// Qualified free function name (`foo`, `mod1::bar`) -> ZIPP global slot index.
    pub fn_globals: HashMap<String, u32>,
    /// Qualified function name -> return `NumKind` (for fast numeric expression inference!).
    pub fn_ret_kinds: HashMap<String, NumKind>,
    /// Qualified static / const name -> ZIPP global slot index.
    pub static_globals: HashMap<String, u32>,
    /// Qualified const name -> `NumKind`.
    pub const_kinds: HashMap<String, NumKind>,
    /// Qualified struct name (and short name) -> StructInfo.
    pub structs: HashMap<String, StructInfo>,
    /// Qualified enum name (and short name) -> EnumInfo.
    pub enums: HashMap<String, EnumInfo>,
    /// Per-module `use` aliases: `module_prefix -> (local_alias -> (is_crate_rooted, target_segments))`.
    pub use_aliases: HashMap<String, HashMap<String, (bool, Vec<String>)>>,
    /// Per-module `use Enum::*;` glob imports: `module_prefix -> Vec<(is_crate_rooted, Vec<String>)>`.
    pub use_globs: HashMap<String, Vec<(bool, Vec<String>)>>,
    /// User-defined `macro_rules!` definitions by name.
    pub macro_rules: HashMap<String, Vec<MacroRule>>,
    /// Method names in this crate that take `&mut self` (so receivers calling them are boxed!).
    pub mut_self_methods: HashSet<String>,
    /// Qualified module names present in this crate (e.g. `"issue_tracker::routes::auth"`).
    pub modules: HashSet<String>,
    /// Optional sibling `src/lib.rs` crate module name (e.g. `"issue_tracker"`),
    /// so `crate::foo` inside `lib.rs` submodules resolves to `issue_tracker::foo`!
    pub lib_crate_name: Option<String>,
}

impl CrateSymbols {
    pub fn collect(
        items: &[Item],
        is_test_mode: bool,
        lib_crate_name: Option<String>,
        alloc_global: &mut impl FnMut(String) -> u32,
    ) -> Self {
        let mut syms = Self {
            lib_crate_name,
            ..Self::default()
        };
        syms.collect_module("", items, is_test_mode, alloc_global);
        syms
    }

    fn collect_module(
        &mut self,
        mod_prefix: &str,
        items: &[Item],
        is_test_mode: bool,
        alloc_global: &mut impl FnMut(String) -> u32,
    ) {
        for item in items {
            if !item_cfg_enabled(&item.attrs, is_test_mode) {
                continue;
            }
            let derives = extract_derives(&item.attrs);
            match &item.kind {
                ItemKind::Fn(f) => {
                    let qname = qualify(mod_prefix, &f.name);
                    let gname = format!("__rs_fn_{}", mangle(&qname));
                    let slot = alloc_global(gname);
                    self.fn_globals.insert(qname.clone(), slot);
                    let rkind = f
                        .ret_type
                        .as_ref()
                        .map(NumKind::from_type)
                        .unwrap_or(NumKind::Unknown);
                    self.fn_ret_kinds.insert(qname, rkind);
                }
                ItemKind::Impl(imp) => {
                    for ii in &imp.items {
                        if let ItemKind::Fn(f) = &ii.kind {
                            if f.params.first().is_some_and(|p| p.is_self && p.is_mut_self) {
                                self.mut_self_methods.insert(f.name.clone());
                            }
                        }
                    }
                }
                ItemKind::Trait(tr) => {
                    for ti in &tr.items {
                        if let ItemKind::Fn(f) = &ti.kind {
                            if f.params.first().is_some_and(|p| p.is_self && p.is_mut_self) {
                                self.mut_self_methods.insert(f.name.clone());
                            }
                        }
                    }
                }
                ItemKind::Struct(s) => {
                    let qname = qualify(mod_prefix, &s.name);
                    let (is_tuple, is_unit, fields) = match &s.fields {
                        StructFields::Named(fs) => (
                            false,
                            false,
                            fs.iter()
                                .map(|f| (f.name.clone(), type_base_name(&f.ty)))
                                .collect(),
                        ),
                        StructFields::Tuple(fs) => (
                            true,
                            false,
                            fs.iter()
                                .enumerate()
                                .map(|(i, f)| (i.to_string(), type_base_name(&f.ty)))
                                .collect(),
                        ),
                        StructFields::Unit => (false, true, Vec::new()),
                    };
                    let info = StructInfo {
                        name: s.name.clone(),
                        is_tuple,
                        is_unit,
                        fields,
                        derives,
                    };
                    self.structs.insert(qname, info.clone());
                    self.structs.entry(s.name.clone()).or_insert(info);
                }
                ItemKind::Enum(e) => {
                    let qname = qualify(mod_prefix, &e.name);
                    let mut variants = HashMap::new();
                    for v in &e.variants {
                        let vk = match &v.fields {
                            StructFields::Unit => VariantKind::Unit,
                            StructFields::Tuple(_) => VariantKind::Tuple,
                            StructFields::Named(_) => VariantKind::Struct,
                        };
                        variants.insert(v.name.clone(), vk);
                    }
                    let info = EnumInfo {
                        name: e.name.clone(),
                        variants,
                        derives,
                    };
                    self.enums.insert(qname, info.clone());
                    self.enums.entry(e.name.clone()).or_insert(info);
                }
                ItemKind::Const(c) => {
                    let qname = qualify(mod_prefix, &c.name);
                    let slot = alloc_global(format!("__rs_const_{}", mangle(&qname)));
                    self.static_globals.insert(qname.clone(), slot);
                    self.const_kinds.insert(qname, NumKind::from_type(&c.ty));
                }
                ItemKind::Static(s) => {
                    let qname = qualify(mod_prefix, &s.name);
                    let slot = alloc_global(format!("__rs_static_{}", mangle(&qname)));
                    self.static_globals.insert(qname.clone(), slot);
                    self.const_kinds.insert(qname, NumKind::from_type(&s.ty));
                }
                ItemKind::Mod(m) => {
                    if let Some(sub_items) = &m.items {
                        let sub = qualify(mod_prefix, &m.name);
                        self.modules.insert(sub.clone());
                        self.collect_module(&sub, sub_items, is_test_mode, alloc_global);
                    }
                }
                ItemKind::Use(tree) => {
                    self.collect_use(mod_prefix, &[], tree);
                }
                ItemKind::MacroRules(mr) => {
                    self.macro_rules.insert(mr.name.clone(), mr.rules.clone());
                }
                ItemKind::TypeAlias(ta) => {
                    if let Some(Type {
                        kind: TypeKind::Path(p),
                        ..
                    }) = &ta.target
                    {
                        if !p.segments.is_empty()
                            && !matches!(ta.name.as_str(), "Result" | "Option")
                        {
                            let raw_segs: Vec<String> =
                                p.segments.iter().map(|s| s.ident.clone()).collect();
                            let rooted = matches!(
                                raw_segs.first().map(|s| s.as_str()),
                                Some("crate" | "self" | "super")
                            ) || self.lib_crate_name.as_deref()
                                == raw_segs.first().map(|s| s.as_str());
                            let resolved = resolve_mod_relative(
                                mod_prefix,
                                &raw_segs,
                                self.lib_crate_name.as_deref(),
                            );
                            self.use_aliases
                                .entry(mod_prefix.to_owned())
                                .or_default()
                                .entry(ta.name.clone())
                                .or_insert((rooted, resolved));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn collect_use(&mut self, mod_prefix: &str, base: &[String], tree: &UseTree) {
        let mut full = base.to_vec();
        full.extend(tree.prefix.iter().cloned());
        match &tree.kind {
            UseTreeKind::Simple(alias) => {
                if full.is_empty() {
                    return;
                }
                let (target, local_name) = if full.last().map(|s| s.as_str()) == Some("self") {
                    let trimmed = full[..full.len() - 1].to_vec();
                    let name = alias
                        .clone()
                        .or_else(|| trimmed.last().cloned())
                        .unwrap_or_default();
                    (trimmed, name)
                } else {
                    let name = alias
                        .clone()
                        .or_else(|| full.last().cloned())
                        .unwrap_or_default();
                    (full, name)
                };
                if !local_name.is_empty() && !target.is_empty() {
                    let rooted = matches!(
                        target.first().map(|s| s.as_str()),
                        Some("crate" | "self" | "super")
                    ) || self.lib_crate_name.as_deref()
                        == target.first().map(|s| s.as_str());
                    let resolved = resolve_mod_relative(mod_prefix, &target, self.lib_crate_name.as_deref());
                    self.use_aliases
                        .entry(mod_prefix.to_owned())
                        .or_default()
                        .insert(local_name, (rooted, resolved));
                }
            }
            UseTreeKind::Glob => {
                let rooted = matches!(
                    full.first().map(|s| s.as_str()),
                    Some("crate" | "self" | "super")
                ) || self.lib_crate_name.as_deref()
                    == full.first().map(|s| s.as_str());
                let resolved = resolve_mod_relative(mod_prefix, &full, self.lib_crate_name.as_deref());
                self.use_globs
                    .entry(mod_prefix.to_owned())
                    .or_default()
                    .push((rooted, resolved));
            }
            UseTreeKind::Group(children) => {
                for ch in children {
                    self.collect_use(mod_prefix, &full, ch);
                }
            }
        }
    }

    fn expand_if_child_mod(&self, mp: &str, segs: &[String]) -> Vec<String> {
        if !mp.is_empty() && !segs.is_empty() {
            let child = qualify(mp, &segs[0]);
            if self.modules.contains(&child) {
                let mut out: Vec<String> = mp.split("::").map(str::to_owned).collect();
                out.extend_from_slice(segs);
                return out;
            }
        }
        segs.to_vec()
    }

    fn item_exists(&self, q: &str) -> bool {
        self.fn_globals.contains_key(q)
            || self.static_globals.contains_key(q)
            || self.structs.contains_key(q)
            || self.enums.contains_key(q)
            || self.modules.contains(q)
    }

    /// Resolve a path written inside `mod_prefix` through `use` aliases, `pub use`
    /// cross-module re-exports (named & glob), child submodules, `crate::`, `super::`, and `self::`.
    pub fn resolve_path(&self, mod_prefix: &str, raw: &[String]) -> Vec<String> {
        if raw.is_empty() {
            return Vec::new();
        }
        let mut cur = if let Some(aliases) = self.use_aliases.get(mod_prefix) {
            if let Some((rooted, target)) = aliases.get(&raw[0]) {
                let mut out = if *rooted {
                    target.clone()
                } else {
                    self.expand_if_child_mod(mod_prefix, target)
                };
                out.extend_from_slice(&raw[1..]);
                out
            } else {
                let r = resolve_mod_relative(mod_prefix, raw, self.lib_crate_name.as_deref());
                if !matches!(raw[0].as_str(), "crate" | "self" | "super") {
                    self.expand_if_child_mod(mod_prefix, &r)
                } else {
                    r
                }
            }
        } else {
            let r = resolve_mod_relative(mod_prefix, raw, self.lib_crate_name.as_deref());
            if !matches!(raw[0].as_str(), "crate" | "self" | "super") {
                self.expand_if_child_mod(mod_prefix, &r)
            } else {
                r
            }
        };

        // Check glob imports `use foo::*;` for single-segment names
        if raw.len() == 1 && cur.len() == 1 && cur[0] == raw[0] {
            if let Some(globs) = self.use_globs.get(mod_prefix) {
                for (rooted, g) in globs {
                    let g_exp = if *rooted {
                        g.clone()
                    } else {
                        self.expand_if_child_mod(mod_prefix, g)
                    };
                    let mut cand = g_exp.clone();
                    cand.push(raw[0].clone());
                    let q = cand.join("::");
                    if self.item_exists(&q) {
                        cur = cand;
                        break;
                    }
                    if let Some(ename) = g_exp.last() {
                        if let Some(einfo) = self.enums.get(ename) {
                            if einfo.variants.contains_key(&raw[0]) {
                                cur = cand;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Follow cross-module `pub use` re-exports (named & glob)! E.g. `models::User` -> `models::user::User`
        for _ in 0..8 {
            let mut changed = false;
            if cur.len() >= 2 {
                for k in (1..cur.len()).rev() {
                    let mp = cur[..k].join("::");
                    if let Some(mod_aliases) = self.use_aliases.get(&mp) {
                        if let Some((rooted, target)) = mod_aliases.get(&cur[k]) {
                            let mut next = if *rooted {
                                target.clone()
                            } else {
                                self.expand_if_child_mod(&mp, target)
                            };
                            next.extend_from_slice(&cur[k + 1..]);
                            if next != cur {
                                cur = next;
                                changed = true;
                                break;
                            }
                        }
                    }
                    if let Some(mod_globs) = self.use_globs.get(&mp) {
                        for (rooted, g) in mod_globs {
                            let g_exp = if *rooted {
                                g.clone()
                            } else {
                                self.expand_if_child_mod(&mp, g)
                            };
                            let mut cand = g_exp;
                            cand.push(cur[k].clone());
                            let q = cand.join("::");
                            if self.item_exists(&q) {
                                cand.extend_from_slice(&cur[k + 1..]);
                                if cand != cur {
                                    cur = cand;
                                    changed = true;
                                    break;
                                }
                            }
                        }
                        if changed {
                            break;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        cur
    }

    pub fn lookup_fn_global(&self, mod_prefix: &str, raw: &[String]) -> Option<(String, u32)> {
        let resolved = self.resolve_path(mod_prefix, raw);
        let q = resolved.join("::");
        if let Some(&slot) = self.fn_globals.get(&q) {
            return Some((q, slot));
        }
        if !mod_prefix.is_empty() && raw.len() == 1 {
            let local_q = qualify(mod_prefix, &raw[0]);
            if let Some(&slot) = self.fn_globals.get(&local_q) {
                return Some((local_q, slot));
            }
        }
        None
    }

    pub fn lookup_static_global(&self, mod_prefix: &str, raw: &[String]) -> Option<(String, u32)> {
        let resolved = self.resolve_path(mod_prefix, raw);
        let q = resolved.join("::");
        if let Some(&slot) = self.static_globals.get(&q) {
            return Some((q, slot));
        }
        if !mod_prefix.is_empty() && raw.len() == 1 {
            let local_q = qualify(mod_prefix, &raw[0]);
            if let Some(&slot) = self.static_globals.get(&local_q) {
                return Some((local_q, slot));
            }
        }
        None
    }

    pub fn lookup_enum_variant(&self, mod_prefix: &str, raw: &[String]) -> Option<(String, String, VariantKind)> {
        let resolved = self.resolve_path(mod_prefix, raw);
        if resolved.len() >= 2 {
            let var = resolved.last().unwrap();
            let enum_full = resolved[..resolved.len() - 1].join("::");
            let enum_short = &resolved[resolved.len() - 2];
            if let Some(info) = self.enums.get(&enum_full).or_else(|| self.enums.get(enum_short)) {
                if let Some(&vk) = info.variants.get(var) {
                    return Some((info.name.clone(), var.clone(), vk));
                }
            }
        }
        None
    }

    /// Like `type_desc(ty)`, but resolves user-defined structs/enums in `mod_prefix`
    /// to their unique module-qualified name (e.g. `http::tasks::CreateBody`),
    /// preventing collisions when multiple modules define `Q`, `ListQ`, or `CreateBody`.
    pub fn type_desc_resolved(&self, mod_prefix: &str, ty: &Type) -> String {
        match &ty.kind {
            TypeKind::Path(p) => {
                let Some(last) = p.segments.last() else {
                    return String::new();
                };
                let raw: Vec<String> = p.segments.iter().map(|s| s.ident.clone()).collect();
                let resolved = self.resolve_path(mod_prefix, &raw);
                let q = resolved.join("::");
                let base = if !mod_prefix.is_empty() && resolved.len() == 1 {
                    let local_q = qualify(mod_prefix, &resolved[0]);
                    if self.structs.contains_key(&local_q) || self.enums.contains_key(&local_q) {
                        local_q
                    } else if self.structs.contains_key(&q) || self.enums.contains_key(&q) {
                        q
                    } else {
                        last.ident.clone()
                    }
                } else if self.structs.contains_key(&q) || self.enums.contains_key(&q) {
                    q
                } else {
                    last.ident.clone()
                };
                if last.type_args.is_empty() {
                    base
                } else {
                    let inner: Vec<String> = last
                        .type_args
                        .iter()
                        .map(|t| self.type_desc_resolved(mod_prefix, t))
                        .collect();
                    format!("{base}<{}>", inner.join(", "))
                }
            }
            TypeKind::Ref { inner, .. } | TypeKind::Ptr { inner, .. } => {
                self.type_desc_resolved(mod_prefix, inner)
            }
            TypeKind::Tuple(v) if v.is_empty() => "()".to_owned(),
            TypeKind::Tuple(v) => {
                let inner: Vec<String> = v
                    .iter()
                    .map(|t| self.type_desc_resolved(mod_prefix, t))
                    .collect();
                format!("({})", inner.join(", "))
            }
            TypeKind::Array(elem, _) | TypeKind::Slice(elem) => {
                format!("Vec<{}>", self.type_desc_resolved(mod_prefix, elem))
            }
            _ => String::new(),
        }
    }
}

pub(super) fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}::{name}")
    }
}

pub(super) fn mangle(qname: &str) -> String {
    qname.replace("::", "__")
}

fn resolve_mod_relative(
    mod_prefix: &str,
    segs: &[String],
    lib_crate_name: Option<&str>,
) -> Vec<String> {
    if segs.is_empty() {
        return Vec::new();
    }
    match segs[0].as_str() {
        "crate" => {
            if let Some(lib_name) = lib_crate_name {
                if mod_prefix == lib_name
                    || mod_prefix
                        .strip_prefix(lib_name)
                        .is_some_and(|r| r.starts_with("::"))
                {
                    let mut out = vec![lib_name.to_owned()];
                    out.extend_from_slice(&segs[1..]);
                    return out;
                }
            }
            segs[1..].to_vec()
        }
        "self" => {
            let mut base: Vec<String> = if mod_prefix.is_empty() {
                Vec::new()
            } else {
                mod_prefix.split("::").map(str::to_owned).collect()
            };
            base.extend_from_slice(&segs[1..]);
            base
        }
        "super" => {
            let mut base: Vec<String> = if mod_prefix.is_empty() {
                Vec::new()
            } else {
                mod_prefix.split("::").map(str::to_owned).collect()
            };
            let mut idx = 0;
            while segs.get(idx).map(|s| s.as_str()) == Some("super") {
                base.pop();
                idx += 1;
            }
            base.extend_from_slice(&segs[idx..]);
            base
        }
        _ => segs.to_vec(),
    }
}

pub(super) fn extract_derives(attrs: &[Attribute]) -> Vec<String> {
    let mut out = Vec::new();
    for a in attrs {
        if a.path.last().map(|s| s.as_str()) == Some("derive") {
            if let AttrArgs::Delimited(toks) = &a.args {
                for t in toks {
                    if let zipp_rsparse::token::TokenKind::Ident(name) = &t.kind {
                        out.push(name.clone());
                    }
                }
            }
        }
    }
    out
}

pub(super) fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path.last().map(|s| s.as_str()) == Some(name))
}

// ---- Per-function analysis: boxed vars (`&mut x`, closure captures) & free captures ----

#[derive(Default)]
pub(super) struct FnAnalysis {
    /// Local names in THIS function that must be stored in a `{ __rs: 6, v }` ref cell.
    pub boxed_vars: HashSet<String>,
    /// Free variables referenced inside this closure/fn that are not declared locally.
    pub free_vars: Vec<String>,
}

pub(super) fn analyze_fn(
    params: &[Pat],
    body: &Block,
    mut_self_methods: &HashSet<String>,
) -> FnAnalysis {
    let mut declared = HashSet::new();
    for p in params {
        collect_pat_names(p, &mut declared);
    }
    let mut boxed = HashSet::new();
    let mut free = Vec::new();
    let mut free_seen = HashSet::new();
    scan_block(
        body,
        &mut declared,
        &mut boxed,
        &mut free,
        &mut free_seen,
        mut_self_methods,
    );
    FnAnalysis {
        boxed_vars: boxed,
        free_vars: free,
    }
}

pub(super) fn analyze_closure(
    params: &[Pat],
    body: &Expr,
    mut_self_methods: &HashSet<String>,
) -> FnAnalysis {
    let mut declared = HashSet::new();
    for p in params {
        collect_pat_names(p, &mut declared);
    }
    let mut boxed = HashSet::new();
    let mut free = Vec::new();
    let mut free_seen = HashSet::new();
    scan_expr(
        body,
        &mut declared,
        &mut boxed,
        &mut free,
        &mut free_seen,
        mut_self_methods,
    );
    FnAnalysis {
        boxed_vars: boxed,
        free_vars: free,
    }
}

pub(super) fn collect_pat_names(pat: &Pat, out: &mut HashSet<String>) {
    match &pat.kind {
        PatKind::Ident { name, subpat, .. } => {
            out.insert(name.clone());
            if let Some(sub) = subpat {
                collect_pat_names(sub, out);
            }
        }
        PatKind::Tuple(ps) | PatKind::Slice(ps) | PatKind::Or(ps) => {
            for p in ps {
                collect_pat_names(p, out);
            }
        }
        PatKind::TupleStruct { elems, .. } => {
            for p in elems {
                collect_pat_names(p, out);
            }
        }
        PatKind::Struct { fields, .. } => {
            for f in fields {
                collect_pat_names(&f.pat, out);
            }
        }
        PatKind::Ref { inner, .. } => collect_pat_names(inner, out),
        _ => {}
    }
}

fn scan_block(
    block: &Block,
    declared: &mut HashSet<String>,
    boxed: &mut HashSet<String>,
    free: &mut Vec<String>,
    free_seen: &mut HashSet<String>,
    mut_self_methods: &HashSet<String>,
) {
    let mut inner_decl = declared.clone();
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let {
                pat,
                init,
                else_block,
                ..
            } => {
                if let Some(e) = init {
                    scan_expr(e, &mut inner_decl, boxed, free, free_seen, mut_self_methods);
                }
                if let Some(eb) = else_block {
                    scan_block(eb, &mut inner_decl, boxed, free, free_seen, mut_self_methods);
                }
                collect_pat_names(pat, &mut inner_decl);
            }
            StmtKind::Expr { expr, .. } => {
                scan_expr(expr, &mut inner_decl, boxed, free, free_seen, mut_self_methods);
            }
            _ => {}
        }
    }
    if let Some(tail) = &block.tail {
        scan_expr(tail, &mut inner_decl, boxed, free, free_seen, mut_self_methods);
    }
}

fn scan_if_cond(
    cond: &IfCond,
    declared: &mut HashSet<String>,
    boxed: &mut HashSet<String>,
    free: &mut Vec<String>,
    free_seen: &mut HashSet<String>,
    mut_self_methods: &HashSet<String>,
) {
    match cond {
        IfCond::Expr(e) => scan_expr(e, declared, boxed, free, free_seen, mut_self_methods),
        IfCond::Let { pat, expr } => {
            scan_expr(expr, declared, boxed, free, free_seen, mut_self_methods);
            collect_pat_names(pat, declared);
        }
        IfCond::Chain(cs) => {
            for c in cs {
                scan_if_cond(c, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
    }
}

fn scan_expr(
    expr: &Expr,
    declared: &mut HashSet<String>,
    boxed: &mut HashSet<String>,
    free: &mut Vec<String>,
    free_seen: &mut HashSet<String>,
    mut_self_methods: &HashSet<String>,
) {
    match &expr.kind {
        ExprKind::Path(p) => {
            if p.segments.len() == 1 && !p.leading_colon {
                let name = &p.segments[0].ident;
                if !declared.contains(name) && free_seen.insert(name.clone()) {
                    free.push(name.clone());
                }
            }
        }
        ExprKind::Ref { is_mut: true, expr: inner } => {
            if let ExprKind::Path(p) = &inner.kind {
                if p.segments.len() == 1 && !p.leading_colon {
                    boxed.insert(p.segments[0].ident.clone());
                }
            }
            scan_expr(inner, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::Ref { expr: inner, .. }
        | ExprKind::Unary { expr: inner, .. }
        | ExprKind::Cast { expr: inner, .. }
        | ExprKind::Try(inner)
        | ExprKind::Await(inner)
        | ExprKind::Field { receiver: inner, .. }
        | ExprKind::TupleIndex { receiver: inner, .. } => {
            scan_expr(inner, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::Binary { left, right, .. }
        | ExprKind::Assign { left, right, .. }
        | ExprKind::Index {
            receiver: left,
            index: right,
        }
        | ExprKind::Repeat {
            elem: left,
            count: right,
        } => {
            scan_expr(left, declared, boxed, free, free_seen, mut_self_methods);
            scan_expr(right, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                scan_expr(s, declared, boxed, free, free_seen, mut_self_methods);
            }
            if let Some(e) = end {
                scan_expr(e, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::Tuple(es) | ExprKind::Array(es) => {
            for e in es {
                scan_expr(e, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::Struct { fields, rest, .. } => {
            for f in fields {
                scan_expr(&f.value, declared, boxed, free, free_seen, mut_self_methods);
            }
            if let Some(r) = rest {
                scan_expr(r, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => {
            scan_block(b, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::AsyncBlock { block, .. } => {
            let inner_an = analyze_fn(&[], block, mut_self_methods);
            for fv in inner_an.free_vars {
                if declared.contains(&fv) {
                    boxed.insert(fv);
                } else if free_seen.insert(fv.clone()) {
                    free.push(fv);
                }
            }
        }
        ExprKind::Closure { params, body, .. } => {
            let pats: Vec<Pat> = params.iter().map(|(p, _)| p.clone()).collect();
            let inner_an = analyze_closure(&pats, body, mut_self_methods);
            for fv in inner_an.free_vars {
                if declared.contains(&fv) {
                    boxed.insert(fv);
                } else if free_seen.insert(fv.clone()) {
                    free.push(fv);
                }
            }
        }
        ExprKind::Call { callee, args } => {
            scan_expr(callee, declared, boxed, free, free_seen, mut_self_methods);
            for a in args {
                scan_expr(a, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // Methods like `opt.take()`, `opt.replace(v)`, or any `&mut self` user method
            // may reassign `*self = ...`, so box the receiver local!
            if matches!(method.as_str(), "take" | "replace") || mut_self_methods.contains(method) {
                if let ExprKind::Path(p) = &receiver.kind {
                    if p.segments.len() == 1 && !p.leading_colon {
                        boxed.insert(p.segments[0].ident.clone());
                    }
                }
            }
            scan_expr(receiver, declared, boxed, free, free_seen, mut_self_methods);
            for a in args {
                scan_expr(a, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::If {
            cond,
            then_block,
            else_branch,
        } => {
            let mut then_decl = declared.clone();
            scan_if_cond(cond, &mut then_decl, boxed, free, free_seen, mut_self_methods);
            scan_block(then_block, &mut then_decl, boxed, free, free_seen, mut_self_methods);
            if let Some(eb) = else_branch {
                scan_expr(eb, declared, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            scan_expr(scrutinee, declared, boxed, free, free_seen, mut_self_methods);
            for arm in arms {
                let mut arm_decl = declared.clone();
                collect_pat_names(&arm.pat, &mut arm_decl);
                if let Some(g) = &arm.guard {
                    scan_expr(g, &mut arm_decl, boxed, free, free_seen, mut_self_methods);
                }
                scan_expr(&arm.body, &mut arm_decl, boxed, free, free_seen, mut_self_methods);
            }
        }
        ExprKind::Loop { body, .. } => {
            scan_block(body, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::While { cond, body, .. } => {
            let mut loop_decl = declared.clone();
            scan_if_cond(cond, &mut loop_decl, boxed, free, free_seen, mut_self_methods);
            scan_block(body, &mut loop_decl, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::For { pat, iter, body, .. } => {
            scan_expr(iter, declared, boxed, free, free_seen, mut_self_methods);
            let mut loop_decl = declared.clone();
            collect_pat_names(pat, &mut loop_decl);
            scan_block(body, &mut loop_decl, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::Break { value: Some(v), .. } | ExprKind::Return(Some(v)) => {
            scan_expr(v, declared, boxed, free, free_seen, mut_self_methods);
        }
        ExprKind::MacroCall(mc) => {
            for tok in &mc.tokens {
                match &tok.kind {
                    zipp_rsparse::token::TokenKind::Ident(id) => {
                        if !declared.contains(id) && free_seen.insert(id.clone()) {
                            free.push(id.clone());
                        }
                    }
                    zipp_rsparse::token::TokenKind::Str(s) => {
                        for cap in extract_format_captures(s) {
                            if !declared.contains(&cap) && free_seen.insert(cap.clone()) {
                                free.push(cap);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Extract Rust 1.58+ inline format captures like `"{name}"`, `"{count:?}"` from a format string.
pub(super) fn extract_format_captures(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            if let Some(rel_end) = s[i + 1..].find('}') {
                let inside = &s[i + 1..i + 1 + rel_end];
                let key = inside.split(':').next().unwrap_or("");
                if !key.is_empty()
                    && !key.bytes().all(|b| b.is_ascii_digit())
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    out.push(key.to_owned());
                }
                i += 2 + rel_end;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub(super) fn literal_num_kind(lit: &Literal) -> NumKind {
    match lit {
        Literal::Int(_, IntSuffix::I128 | IntSuffix::U128) => NumKind::Unknown,
        Literal::Int(v, _) if *v <= 9_007_199_254_740_991 => NumKind::Int,
        Literal::Int(_, _) => NumKind::Unknown,
        Literal::Float(_, FloatSuffix::None | FloatSuffix::F32 | FloatSuffix::F64) => NumKind::Float,
        Literal::Byte(_) => NumKind::Int,
        _ => NumKind::Unknown,
    }
}
