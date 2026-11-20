//! PHP namespace and `use` import resolution at compile time.

use std::collections::HashMap;
use zipp_phpparse::ast::{Name, NameKind, TypeHint, UseItem, UseKind};

#[derive(Debug, Clone, Default)]
pub(super) struct NsResolver {
    pub namespace: String,
    /// lowercase alias -> FQN (without leading `\`)
    class_uses: HashMap<String, String>,
    fn_uses: HashMap<String, String>,
    const_uses: HashMap<String, String>,
}

impl NsResolver {
    pub fn enter_namespace(&mut self, parts: Option<&[String]>) {
        self.namespace = parts.map(|p| p.join("\\")).unwrap_or_default();
        self.class_uses.clear();
        self.fn_uses.clear();
        self.const_uses.clear();
    }

    pub fn add_use(&mut self, item: &UseItem) {
        let fqn = item.name.join("\\");
        let alias = item
            .alias
            .clone()
            .or_else(|| item.name.last().cloned())
            .unwrap_or_default();
        if alias.is_empty() {
            return;
        }
        match item.kind {
            UseKind::Normal => {
                self.class_uses.insert(alias.to_ascii_lowercase(), fqn);
            }
            UseKind::Function => {
                self.fn_uses.insert(alias.to_ascii_lowercase(), fqn);
            }
            UseKind::Const => {
                self.const_uses.insert(alias, fqn);
            }
        }
    }

    /// Qualify a declared top-level symbol in the current namespace.
    pub fn qualify_decl(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_owned()
        } else {
            format!("{}\\{name}", self.namespace)
        }
    }

    /// Resolve a class/interface/trait/enum name (no fallback to global).
    pub fn resolve_class(&self, name: &Name) -> String {
        let joined = name.joined();
        if name.kind == NameKind::FullyQualified {
            return joined;
        }
        if name.kind == NameKind::Relative {
            return self.qualify_decl(&joined);
        }
        if name.parts.len() == 1 {
            let lo = name.parts[0].to_ascii_lowercase();
            if matches!(lo.as_str(), "self" | "parent" | "static") {
                return name.parts[0].clone();
            }
            if let Some(target) = self.class_uses.get(&lo) {
                return target.clone();
            }
            return self.qualify_decl(&name.parts[0]);
        }
        // Multi-segment qualified name: check if first segment matches a `use` alias.
        let first_lo = name.parts[0].to_ascii_lowercase();
        if let Some(target) = self.class_uses.get(&first_lo) {
            let rest = name.parts[1..].join("\\");
            return format!("{target}\\{rest}");
        }
        self.qualify_decl(&joined)
    }

    /// Resolve a function call name. Returns `(primary_fqn, optional_global_fallback)`.
    pub fn resolve_function(&self, name: &Name) -> (String, Option<String>) {
        let joined = name.joined();
        if name.kind == NameKind::FullyQualified {
            return (joined, None);
        }
        if name.kind == NameKind::Relative {
            return (self.qualify_decl(&joined), None);
        }
        if name.parts.len() == 1 {
            let lo = name.parts[0].to_ascii_lowercase();
            if let Some(target) = self.fn_uses.get(&lo) {
                return (target.clone(), None);
            }
            if self.namespace.is_empty() {
                return (joined, None);
            }
            return (self.qualify_decl(&joined), Some(joined));
        }
        let first_lo = name.parts[0].to_ascii_lowercase();
        if let Some(target) = self.class_uses.get(&first_lo) {
            let rest = name.parts[1..].join("\\");
            return (format!("{target}\\{rest}"), None);
        }
        (self.qualify_decl(&joined), None)
    }

    /// Resolve a constant fetch name. Returns `(primary_fqn, optional_global_fallback)`.
    pub fn resolve_const(&self, name: &Name) -> (String, Option<String>) {
        let joined = name.joined();
        if name.kind == NameKind::FullyQualified {
            return (joined, None);
        }
        if name.kind == NameKind::Relative {
            return (self.qualify_decl(&joined), None);
        }
        if name.parts.len() == 1 {
            if let Some(target) = self.const_uses.get(&name.parts[0]) {
                return (target.clone(), None);
            }
            if self.namespace.is_empty() {
                return (joined, None);
            }
            return (self.qualify_decl(&joined), Some(joined));
        }
        let first_lo = name.parts[0].to_ascii_lowercase();
        if let Some(target) = self.class_uses.get(&first_lo) {
            let rest = name.parts[1..].join("\\");
            return (format!("{target}\\{rest}"), None);
        }
        (self.qualify_decl(&joined), None)
    }

    /// Render a PHP 8 type hint into its canonical runtime string (`?int`, `int|string`, `App\User`).
    pub fn format_type(&self, ty: &TypeHint) -> String {
        match ty {
            TypeHint::Simple(n) => {
                if n.kind == NameKind::Unqualified && n.parts.len() == 1 {
                    let lo = n.parts[0].to_ascii_lowercase();
                    if matches!(
                        lo.as_str(),
                        "int"
                            | "float"
                            | "string"
                            | "bool"
                            | "array"
                            | "object"
                            | "callable"
                            | "iterable"
                            | "mixed"
                            | "void"
                            | "never"
                            | "null"
                            | "true"
                            | "false"
                            | "self"
                            | "parent"
                            | "static"
                    ) {
                        return lo;
                    }
                }
                self.resolve_class(n)
            }
            TypeHint::Nullable(inner) => format!("?{}", self.format_type(inner)),
            TypeHint::Union(list) => list
                .iter()
                .map(|t| self.format_type(t))
                .collect::<Vec<_>>()
                .join("|"),
            TypeHint::Intersection(list) => list
                .iter()
                .map(|t| self.format_type(t))
                .collect::<Vec<_>>()
                .join("&"),
        }
    }
}
