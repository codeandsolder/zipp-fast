//! Item parsing (`fn`, `struct`, `enum`, `impl`, `trait`, `mod`, `use`, `const`, `static`, `type`, `macro_rules!`).

use super::Parser;
use crate::ast::*;
use crate::token::{Span, Token, TokenKind};
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn can_start_item_body(&self) -> bool {
        match self.peek_kind() {
            TokenKind::Pub
            | TokenKind::Fn
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Impl
            | TokenKind::Trait
            | TokenKind::Static
            | TokenKind::Type
            | TokenKind::Mod
            | TokenKind::Use
            | TokenKind::Extern => true,
            TokenKind::Const => matches!(
                self.peek_at(1),
                TokenKind::Ident(_) | TokenKind::Fn | TokenKind::Unsafe | TokenKind::Extern
            ),
            TokenKind::Async => matches!(self.peek_at(1), TokenKind::Fn | TokenKind::Unsafe),
            TokenKind::Unsafe => matches!(
                self.peek_at(1),
                TokenKind::Fn | TokenKind::Trait | TokenKind::Impl | TokenKind::Extern
            ),
            TokenKind::Ident(s) if s == "macro_rules" && matches!(self.peek_at(1), TokenKind::Bang) => {
                true
            }
            _ => false,
        }
    }

    pub(crate) fn parse_item(&mut self) -> Result<Item, ParseError> {
        let start = self.peek().span;
        let attrs = self.parse_outer_attrs()?;
        self.parse_item_with_attrs(attrs, start)
    }

    pub(crate) fn parse_item_with_attrs(
        &mut self,
        attrs: Vec<Attribute>,
        start: Span,
    ) -> Result<Item, ParseError> {
        self.enter()?;
        let res = self.parse_item_inner(attrs, start);
        self.leave();
        res
    }

    fn parse_item_inner(&mut self, attrs: Vec<Attribute>, start: Span) -> Result<Item, ParseError> {
        let vis = self.parse_visibility()?;

        // `macro_rules! name { ... }`
        if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "macro_rules")
            && matches!(self.peek_at(1), TokenKind::Bang)
        {
            let mr = self.parse_macro_rules()?;
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            return Ok(Item {
                attrs,
                vis,
                kind: ItemKind::MacroRules(mr),
                span: start.merge(end),
            });
        }

        // Qualifiers before `fn`, `trait`, `impl`, `extern`
        let is_const = if self.at(&TokenKind::Const)
            && matches!(self.peek_at(1), TokenKind::Fn | TokenKind::Unsafe | TokenKind::Extern)
        {
            self.bump();
            true
        } else {
            false
        };
        let is_async = if self.at(&TokenKind::Async)
            && matches!(self.peek_at(1), TokenKind::Fn | TokenKind::Unsafe)
        {
            self.bump();
            true
        } else {
            false
        };
        let is_unsafe = if self.at(&TokenKind::Unsafe)
            && matches!(
                self.peek_at(1),
                TokenKind::Fn | TokenKind::Trait | TokenKind::Impl | TokenKind::Extern
            ) {
            self.bump();
            true
        } else {
            false
        };

        // Optional `extern "C" fn ...`
        if self.at(&TokenKind::Extern) {
            if matches!(self.peek_at(1), TokenKind::Crate) {
                self.bump();
                self.bump();
                let (name, _) = self.parse_path_ident()?;
                let alias = if self.eat(&TokenKind::As) {
                    let (a, _) = self.parse_ident()?;
                    Some(a)
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "`;`")?;
                let end = self.tokens[self.pos.saturating_sub(1)].span;
                return Ok(Item {
                    attrs,
                    vis,
                    kind: ItemKind::ExternCrate(name, alias),
                    span: start.merge(end),
                });
            }
            if matches!(self.peek_at(1), TokenKind::Fn)
                || (matches!(self.peek_at(1), TokenKind::Str(_))
                    && matches!(self.peek_at(2), TokenKind::Fn))
            {
                self.bump();
                if matches!(self.peek_kind(), TokenKind::Str(_)) {
                    self.bump();
                }
            } else {
                // `extern "C" { ... }` block — skip cleanly
                self.bump();
                if matches!(self.peek_kind(), TokenKind::Str(_)) {
                    self.bump();
                }
                let _ = self.parse_delimited_tokens()?;
                let end = self.tokens[self.pos.saturating_sub(1)].span;
                return Ok(Item {
                    attrs,
                    vis,
                    kind: ItemKind::ExternCrate(String::new(), None),
                    span: start.merge(end),
                });
            }
        }

        let kind = match self.peek_kind() {
            TokenKind::Fn => ItemKind::Fn(self.parse_fn_item(is_async, is_const, is_unsafe)?),
            TokenKind::Struct => ItemKind::Struct(self.parse_struct_item()?),
            TokenKind::Enum => ItemKind::Enum(self.parse_enum_item()?),
            TokenKind::Impl => ItemKind::Impl(self.parse_impl_item()?),
            TokenKind::Trait => ItemKind::Trait(self.parse_trait_item()?),
            TokenKind::Const => ItemKind::Const(self.parse_const_item()?),
            TokenKind::Static => ItemKind::Static(self.parse_static_item()?),
            TokenKind::Type => ItemKind::TypeAlias(self.parse_type_alias_item()?),
            TokenKind::Mod => ItemKind::Mod(self.parse_mod_item()?),
            TokenKind::Use => ItemKind::Use(self.parse_use_item()?),
            _ => {
                // Item-level macro invocation: `foo!(...);` or `foo! { ... }`
                let path = self.parse_path(false)?;
                self.expect(&TokenKind::Bang, "`!`")?;
                let (delim, tokens) = self.parse_delimited_tokens()?;
                if delim != MacroDelim::Brace {
                    self.eat(&TokenKind::Semi);
                }
                let end = self.tokens[self.pos.saturating_sub(1)].span;
                ItemKind::MacroCall(MacroCall {
                    path,
                    delim,
                    tokens,
                    span: start.merge(end),
                })
            }
        };
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        Ok(Item {
            attrs,
            vis,
            kind,
            span: start.merge(end),
        })
    }

    fn parse_fn_item(
        &mut self,
        is_async: bool,
        is_const: bool,
        is_unsafe: bool,
    ) -> Result<FnItem, ParseError> {
        self.expect(&TokenKind::Fn, "`fn`")?;
        let (name, _) = self.parse_ident()?;
        self.skip_generics()?;
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            let pstart = self.peek().span;
            let _pattrs = self.parse_outer_attrs()?;
            if let Some(self_param) = self.try_parse_self_param(pstart)? {
                params.push(self_param);
            } else {
                let pat = self.parse_pat()?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                let pend = self.tokens[self.pos.saturating_sub(1)].span;
                params.push(FnParam {
                    is_self: false,
                    is_mut_self: false,
                    is_ref_self: false,
                    pat,
                    ty,
                    span: pstart.merge(pend),
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let ret_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.skip_where_clause()?;
        let body = if self.eat(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_block()?)
        };
        Ok(FnItem {
            name,
            is_async,
            is_const,
            is_unsafe,
            params,
            ret_type,
            body,
        })
    }

    fn try_parse_self_param(&mut self, start: Span) -> Result<Option<FnParam>, ParseError> {
        let save = self.pos;
        let is_ref = self.eat(&TokenKind::Amp);
        if is_ref && matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
            self.bump();
        }
        let is_mut = self.eat(&TokenKind::Mut);
        if self.eat(&TokenKind::SelfLower) {
            let ty = if self.eat(&TokenKind::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            let span = start.merge(end);
            return Ok(Some(FnParam {
                is_self: true,
                is_mut_self: is_mut,
                is_ref_self: is_ref,
                pat: Pat {
                    kind: PatKind::Ident {
                        is_mut,
                        is_ref: false,
                        name: "self".to_owned(),
                        subpat: None,
                    },
                    span,
                },
                ty,
                span,
            }));
        }
        self.pos = save;
        Ok(None)
    }

    fn parse_struct_item(&mut self) -> Result<StructItem, ParseError> {
        self.expect(&TokenKind::Struct, "`struct`")?;
        let (name, _) = self.parse_ident()?;
        self.skip_generics()?;
        let fields = if self.at(&TokenKind::LParen) {
            let tuple_fields = self.parse_tuple_fields()?;
            self.skip_where_clause()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            StructFields::Tuple(tuple_fields)
        } else {
            self.skip_where_clause()?;
            if self.at(&TokenKind::LBrace) {
                StructFields::Named(self.parse_named_fields()?)
            } else {
                self.expect(&TokenKind::Semi, "`;`")?;
                StructFields::Unit
            }
        };
        Ok(StructItem { name, fields })
    }

    fn parse_named_fields(&mut self) -> Result<Vec<FieldDef>, ParseError> {
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let start = self.peek().span;
            let attrs = self.parse_outer_attrs()?;
            let vis = self.parse_visibility()?;
            let (name, _) = self.parse_ident()?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            fields.push(FieldDef {
                attrs,
                vis,
                name,
                ty,
                span: start.merge(end),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(fields)
    }

    fn parse_tuple_fields(&mut self) -> Result<Vec<TupleFieldDef>, ParseError> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            let start = self.peek().span;
            let attrs = self.parse_outer_attrs()?;
            let vis = self.parse_visibility()?;
            let ty = self.parse_type()?;
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            fields.push(TupleFieldDef {
                attrs,
                vis,
                ty,
                span: start.merge(end),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        Ok(fields)
    }

    fn parse_enum_item(&mut self) -> Result<EnumItem, ParseError> {
        self.expect(&TokenKind::Enum, "`enum`")?;
        let (name, _) = self.parse_ident()?;
        self.skip_generics()?;
        self.skip_where_clause()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let start = self.peek().span;
            let attrs = self.parse_outer_attrs()?;
            let (vname, _) = self.parse_ident()?;
            let fields = if self.at(&TokenKind::LParen) {
                StructFields::Tuple(self.parse_tuple_fields()?)
            } else if self.at(&TokenKind::LBrace) {
                StructFields::Named(self.parse_named_fields()?)
            } else {
                StructFields::Unit
            };
            let discriminant = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            variants.push(EnumVariant {
                attrs,
                name: vname,
                fields,
                discriminant,
                span: start.merge(end),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(EnumItem { name, variants })
    }

    fn parse_impl_item(&mut self) -> Result<ImplItem, ParseError> {
        self.expect(&TokenKind::Impl, "`impl`")?;
        self.skip_generics()?;
        let _const_trait = self.eat(&TokenKind::Const);
        let _neg_trait = self.eat(&TokenKind::Bang);
        let first_ty = self.parse_type()?;
        let (trait_path, self_type) = if self.eat(&TokenKind::For) {
            let tp = match first_ty.kind {
                TypeKind::Path(p) => p,
                _ => {
                    return Err(ParseError::new(
                        "expected trait path before `for` in `impl`",
                        first_ty.span,
                    ))
                }
            };
            let st = self.parse_type()?;
            (Some(tp), st)
        } else {
            (None, first_ty)
        };
        self.skip_where_clause()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        while self.is_inner_attr() {
            let _ = self.parse_attr(true)?;
        }
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            items.push(self.parse_item()?);
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(ImplItem {
            trait_path,
            self_type,
            items,
        })
    }

    fn parse_trait_item(&mut self) -> Result<TraitItem, ParseError> {
        self.expect(&TokenKind::Trait, "`trait`")?;
        let (name, _) = self.parse_ident()?;
        self.skip_generics()?;
        if self.eat(&TokenKind::Colon) {
            let _ = self.parse_trait_bounds()?;
        }
        self.skip_where_clause()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        while self.is_inner_attr() {
            let _ = self.parse_attr(true)?;
        }
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            items.push(self.parse_item()?);
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(TraitItem { name, items })
    }

    fn parse_const_item(&mut self) -> Result<ConstItem, ParseError> {
        self.expect(&TokenKind::Const, "`const`")?;
        let (name, _) = self.parse_ident()?;
        self.expect(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type()?;
        let value = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(ConstItem { name, ty, value })
    }

    fn parse_static_item(&mut self) -> Result<StaticItem, ParseError> {
        self.expect(&TokenKind::Static, "`static`")?;
        let is_mut = self.eat(&TokenKind::Mut);
        let (name, _) = self.parse_ident()?;
        self.expect(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(StaticItem {
            name,
            is_mut,
            ty,
            value,
        })
    }

    fn parse_type_alias_item(&mut self) -> Result<TypeAliasItem, ParseError> {
        self.expect(&TokenKind::Type, "`type`")?;
        let (name, _) = self.parse_ident()?;
        self.skip_generics()?;
        if self.eat(&TokenKind::Colon) {
            let _ = self.parse_trait_bounds()?;
        }
        self.skip_where_clause()?;
        let target = if self.eat(&TokenKind::Eq) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(TypeAliasItem { name, target })
    }

    fn parse_mod_item(&mut self) -> Result<ModItem, ParseError> {
        self.expect(&TokenKind::Mod, "`mod`")?;
        let (name, _) = self.parse_ident()?;
        let items = if self.eat(&TokenKind::Semi) {
            None
        } else {
            self.expect(&TokenKind::LBrace, "`{`")?;
            while self.is_inner_attr() {
                let _ = self.parse_attr(true)?;
            }
            let mut body = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                if self.eat(&TokenKind::Semi) {
                    continue;
                }
                body.push(self.parse_item()?);
            }
            self.expect(&TokenKind::RBrace, "`}`")?;
            Some(body)
        };
        Ok(ModItem { name, items })
    }

    fn parse_use_item(&mut self) -> Result<UseTree, ParseError> {
        self.expect(&TokenKind::Use, "`use`")?;
        let tree = self.parse_use_tree()?;
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(tree)
    }

    fn parse_use_tree(&mut self) -> Result<UseTree, ParseError> {
        let start = self.peek().span;
        let _ = self.eat(&TokenKind::ColonColon);
        let mut prefix = Vec::new();
        loop {
            if self.eat(&TokenKind::Star) {
                let end = self.tokens[self.pos.saturating_sub(1)].span;
                return Ok(UseTree {
                    prefix,
                    kind: UseTreeKind::Glob,
                    span: start.merge(end),
                });
            }
            if self.eat(&TokenKind::LBrace) {
                let mut group = Vec::new();
                while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                    group.push(self.parse_use_tree()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
                return Ok(UseTree {
                    prefix,
                    kind: UseTreeKind::Group(group),
                    span: start.merge(end),
                });
            }
            let (seg, _) = self.parse_path_ident()?;
            prefix.push(seg);
            if self.eat(&TokenKind::ColonColon) {
                continue;
            }
            let alias = if self.eat(&TokenKind::As) {
                let (a, _) = self.parse_ident()?;
                Some(a)
            } else {
                None
            };
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            return Ok(UseTree {
                prefix,
                kind: UseTreeKind::Simple(alias),
                span: start.merge(end),
            });
        }
    }

    fn parse_macro_rules(&mut self) -> Result<MacroRulesItem, ParseError> {
        self.bump(); // `macro_rules`
        self.expect(&TokenKind::Bang, "`!`")?;
        let (name, _) = self.parse_ident()?;
        let (delim, tokens) = self.parse_delimited_tokens()?;
        if delim != MacroDelim::Brace {
            self.eat(&TokenKind::Semi);
        }
        let rules = parse_macro_rules_body(&tokens);
        Ok(MacroRulesItem { name, rules })
    }
}

/// Split the token stream inside `macro_rules! foo { (matcher) => { body }; ... }`
/// into individual `(matcher, body)` arms.
fn parse_macro_rules_body(tokens: &[Token]) -> Vec<MacroRule> {
    let mut rules = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let Some((matcher, next_i)) = slice_delimited(tokens, i) else {
            break;
        };
        i = next_i;
        if !matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::FatArrow)) {
            break;
        }
        i += 1;
        let Some((body, next_i)) = slice_delimited(tokens, i) else {
            break;
        };
        i = next_i;
        if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Semi)) {
            i += 1;
        }
        rules.push(MacroRule { matcher, body });
    }
    rules
}

fn slice_delimited(tokens: &[Token], start: usize) -> Option<(Vec<Token>, usize)> {
    let (open, close) = match tokens.get(start)?.kind {
        TokenKind::LParen => (TokenKind::LParen, TokenKind::RParen),
        TokenKind::LBracket => (TokenKind::LBracket, TokenKind::RBracket),
        TokenKind::LBrace => (TokenKind::LBrace, TokenKind::RBrace),
        _ => return None,
    };
    let mut depth = 1usize;
    let mut i = start + 1;
    let mut out = Vec::new();
    while i < tokens.len() && depth > 0 {
        let tok = &tokens[i];
        if tok.kind == open {
            depth += 1;
            out.push(tok.clone());
        } else if tok.kind == close {
            depth -= 1;
            if depth > 0 {
                out.push(tok.clone());
            }
        } else {
            out.push(tok.clone());
        }
        i += 1;
    }
    Some((out, i))
}
