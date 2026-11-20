//! Function, class, interface, trait, and enum declarations.

use crate::ast::*;
use crate::parser::Parser;
use crate::token::{Span, TokenKind};
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn parse_params(&mut self, allow_promotion: bool) -> Result<Vec<Param>, ParseError> {
        self.expect(&TokenKind::LParen, "'(' starting parameter list")?;
        let mut params = Vec::new();
        while !matches!(self.kind(), TokenKind::RParen | TokenKind::Eof) {
            let pstart = self.span();
            let attributes = self.parse_attributes()?;
            let mut visibility = None;
            let mut set_visibility = None;
            let mut readonly = false;
            if allow_promotion {
                loop {
                    match self.kind() {
                        TokenKind::KwPublic | TokenKind::KwProtected | TokenKind::KwPrivate => {
                            let vis = match self.advance().kind {
                                TokenKind::KwPublic => Visibility::Public,
                                TokenKind::KwProtected => Visibility::Protected,
                                _ => Visibility::Private,
                            };
                            if self.eat_set_modifier() {
                                set_visibility = Some(vis);
                                if visibility.is_none() {
                                    visibility = Some(Visibility::Public);
                                }
                            } else {
                                visibility = Some(vis);
                            }
                        }
                        TokenKind::KwReadonly => {
                            self.advance();
                            readonly = true;
                        }
                        _ => break,
                    }
                }
            }
            // Optional type hint (present when next token is not `&`, `...`, or `$var`).
            let ty = if !matches!(
                self.kind(),
                TokenKind::Amp | TokenKind::Ellipsis | TokenKind::Var(_)
            ) && self.starts_type()
            {
                Some(self.parse_type()?)
            } else {
                None
            };
            let by_ref = self.eat(&TokenKind::Amp);
            let variadic = self.eat(&TokenKind::Ellipsis);
            let (name, _) = self.expect_var()?;
            let default = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            params.push(Param {
                attributes,
                visibility,
                set_visibility,
                readonly,
                ty,
                by_ref,
                variadic,
                name,
                default,
                span: pstart.merge(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')' closing parameter list")?;
        Ok(params)
    }

    pub(crate) fn parse_return_type(&mut self) -> Result<Option<TypeHint>, ParseError> {
        if self.eat(&TokenKind::Colon) {
            Ok(Some(self.parse_type()?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn parse_function_decl(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<FunctionDecl, ParseError> {
        self.expect(&TokenKind::KwFunction, "'function'")?;
        let by_ref = self.eat(&TokenKind::Amp);
        let (name, _) = self.expect_member_name()?;
        let params = self.parse_params(false)?;
        let return_type = self.parse_return_type()?;
        self.expect(&TokenKind::LBrace, "'{' starting function body")?;
        let mut body = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        let end = self.expect(&TokenKind::RBrace, "'}' closing function body")?;
        Ok(FunctionDecl {
            attributes,
            name,
            by_ref,
            params,
            return_type,
            body,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_class_decl(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<ClassDecl, ParseError> {
        let mut is_abstract = false;
        let mut is_final = false;
        let mut is_readonly = false;
        loop {
            match self.kind() {
                TokenKind::KwAbstract => {
                    self.advance();
                    is_abstract = true;
                }
                TokenKind::KwFinal => {
                    self.advance();
                    is_final = true;
                }
                TokenKind::KwReadonly => {
                    self.advance();
                    is_readonly = true;
                }
                _ => break,
            }
        }
        self.expect(&TokenKind::KwClass, "'class'")?;
        let (name, _) = self.expect_member_name()?;
        let extends = if self.eat(&TokenKind::KwExtends) {
            Some(self.parse_name()?)
        } else {
            None
        };
        let mut implements = Vec::new();
        if self.eat(&TokenKind::KwImplements) {
            implements.push(self.parse_name()?);
            while self.eat(&TokenKind::Comma) {
                implements.push(self.parse_name()?);
            }
        }
        let (members, end) = self.parse_class_body()?;
        Ok(ClassDecl {
            attributes,
            is_abstract,
            is_final,
            is_readonly,
            name,
            extends,
            implements,
            members,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_interface_decl(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<InterfaceDecl, ParseError> {
        self.expect(&TokenKind::KwInterface, "'interface'")?;
        let (name, _) = self.expect_member_name()?;
        let mut extends = Vec::new();
        if self.eat(&TokenKind::KwExtends) {
            extends.push(self.parse_name()?);
            while self.eat(&TokenKind::Comma) {
                extends.push(self.parse_name()?);
            }
        }
        let (members, end) = self.parse_class_body()?;
        Ok(InterfaceDecl {
            attributes,
            name,
            extends,
            members,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_trait_decl(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<TraitDecl, ParseError> {
        self.expect(&TokenKind::KwTrait, "'trait'")?;
        let (name, _) = self.expect_member_name()?;
        let (members, end) = self.parse_class_body()?;
        Ok(TraitDecl {
            attributes,
            name,
            members,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_enum_decl(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<EnumDecl, ParseError> {
        self.expect(&TokenKind::KwEnum, "'enum'")?;
        let (name, _) = self.expect_member_name()?;
        let backing_type = if self.eat(&TokenKind::Colon) {
            Some(self.expect_ident()?.0)
        } else {
            None
        };
        let mut implements = Vec::new();
        if self.eat(&TokenKind::KwImplements) {
            implements.push(self.parse_name()?);
            while self.eat(&TokenKind::Comma) {
                implements.push(self.parse_name()?);
            }
        }
        self.expect(&TokenKind::LBrace, "'{' starting enum body")?;
        let mut cases = Vec::new();
        let mut members = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            let mstart = self.span();
            let mattrs = self.parse_attributes()?;
            if self.eat(&TokenKind::KwCase) {
                let (cname, _) = self.expect_member_name()?;
                let value = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                let cend = self.expect_semi()?;
                cases.push(EnumCase {
                    attributes: mattrs,
                    name: cname,
                    value,
                    span: mstart.merge(cend),
                });
            } else {
                members.push(self.parse_class_member(mattrs, mstart)?);
            }
        }
        let end = self.expect(&TokenKind::RBrace, "'}' closing enum body")?;
        Ok(EnumDecl {
            attributes,
            name,
            backing_type,
            implements,
            cases,
            members,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_class_body(&mut self) -> Result<(Vec<ClassMember>, Span), ParseError> {
        self.expect(&TokenKind::LBrace, "'{' starting class body")?;
        let mut members = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            let mstart = self.span();
            let attrs = self.parse_attributes()?;
            if matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                break;
            }
            members.push(self.parse_class_member(attrs, mstart)?);
        }
        let end = self.expect(&TokenKind::RBrace, "'}' closing class body")?;
        Ok((members, end))
    }

    fn parse_class_member(
        &mut self,
        attributes: Vec<Attribute>,
        start: Span,
    ) -> Result<ClassMember, ParseError> {
        if self.eat(&TokenKind::KwUse) {
            let mut traits = vec![self.parse_name()?];
            while self.eat(&TokenKind::Comma) {
                traits.push(self.parse_name()?);
            }
            let mut adaptations = Vec::new();
            if self.eat(&TokenKind::LBrace) {
                while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                    let first = self.parse_name()?;
                    let (trait_name, method) = if self.eat(&TokenKind::DoubleColon) {
                        let (m, _) = self.expect_member_name()?;
                        (Some(first), m)
                    } else {
                        let m = first.parts.into_iter().last().unwrap_or_default();
                        (None, m)
                    };
                    if self.eat(&TokenKind::KwInsteadOf) {
                        let mut instead_of = vec![self.parse_name()?];
                        while self.eat(&TokenKind::Comma) {
                            instead_of.push(self.parse_name()?);
                        }
                        self.expect_semi()?;
                        adaptations.push(TraitAdaptation::Precedence {
                            trait_name,
                            method,
                            instead_of,
                        });
                    } else if self.eat(&TokenKind::KwAs) {
                        let vis = match self.kind() {
                            TokenKind::KwPublic => {
                                self.advance();
                                Some(Visibility::Public)
                            }
                            TokenKind::KwProtected => {
                                self.advance();
                                Some(Visibility::Protected)
                            }
                            TokenKind::KwPrivate => {
                                self.advance();
                                Some(Visibility::Private)
                            }
                            _ => None,
                        };
                        let alias = if !matches!(self.kind(), TokenKind::Semi) {
                            Some(self.expect_member_name()?.0)
                        } else {
                            None
                        };
                        self.expect_semi()?;
                        adaptations.push(TraitAdaptation::Alias {
                            trait_name,
                            method,
                            visibility: vis,
                            alias,
                        });
                    } else {
                        return Err(self.error_here("expected 'insteadof' or 'as' in trait adaptation"));
                    }
                }
                let end = self.expect(&TokenKind::RBrace, "'}' closing trait use")?;
                return Ok(ClassMember::TraitUse {
                    traits,
                    adaptations,
                    span: start.merge(end),
                });
            }
            let end = self.expect_semi()?;
            return Ok(ClassMember::TraitUse {
                traits,
                adaptations,
                span: start.merge(end),
            });
        }

        let mut visibility = Visibility::Public;
        let mut set_visibility = None;
        let mut is_static = false;
        let mut is_abstract = false;
        let mut is_final = false;
        let mut is_readonly = false;

        loop {
            match self.kind() {
                TokenKind::KwVar => {
                    self.advance();
                    visibility = Visibility::Public;
                }
                TokenKind::KwPublic | TokenKind::KwProtected | TokenKind::KwPrivate => {
                    let vis = match self.advance().kind {
                        TokenKind::KwPublic => Visibility::Public,
                        TokenKind::KwProtected => Visibility::Protected,
                        _ => Visibility::Private,
                    };
                    if self.eat_set_modifier() {
                        set_visibility = Some(vis);
                    } else {
                        visibility = vis;
                    }
                }
                TokenKind::KwStatic => {
                    self.advance();
                    is_static = true;
                }
                TokenKind::KwAbstract => {
                    self.advance();
                    is_abstract = true;
                }
                TokenKind::KwFinal => {
                    self.advance();
                    is_final = true;
                }
                TokenKind::KwReadonly => {
                    self.advance();
                    is_readonly = true;
                }
                _ => break,
            }
        }

        // Class constant: `[public] [final] const [Type] NAME = expr;`
        if self.eat(&TokenKind::KwConst) {
            // PHP 8.3 typed class constants: if token after potential type is `=`, it's untyped.
            let ty = if !matches!(self.peek_at(1).kind, TokenKind::Eq)
                && self.starts_type()
            {
                Some(self.parse_type()?)
            } else {
                None
            };
            let mut items = Vec::new();
            loop {
                let (name, _) = self.expect_member_name()?;
                self.expect(&TokenKind::Eq, "'=' in class const")?;
                let val = self.parse_expr()?;
                items.push((name, val));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let end = self.expect_semi()?;
            return Ok(ClassMember::Const {
                attributes,
                visibility,
                is_final,
                ty,
                items,
                span: start.merge(end),
            });
        }

        // Method: `[modifiers] function [&]name(params)[: ret] { ... } | ;`
        if self.eat(&TokenKind::KwFunction) {
            let by_ref = self.eat(&TokenKind::Amp);
            let (name, _) = self.expect_member_name()?;
            let allow_promo = name.eq_ignore_ascii_case("__construct");
            let params = self.parse_params(allow_promo)?;
            let return_type = self.parse_return_type()?;
            let body = if self.eat(&TokenKind::Semi) {
                None
            } else {
                self.expect(&TokenKind::LBrace, "'{' starting method body")?;
                let mut stmts = Vec::new();
                while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                    stmts.push(self.parse_stmt()?);
                }
                self.expect(&TokenKind::RBrace, "'}' closing method body")?;
                Some(stmts)
            };
            return Ok(ClassMember::Method {
                attributes,
                visibility,
                is_static,
                is_abstract,
                is_final,
                by_ref,
                name,
                params,
                return_type,
                body,
                span: start.merge(self.prev_span()),
            });
        }

        // Property: `[modifiers] [Type] $a [= expr] [{ get ...; set ...; }] | , $b;`
        let ty = if !matches!(self.kind(), TokenKind::Var(_)) && self.starts_type() {
            Some(self.parse_type()?)
        } else {
            None
        };
        let mut items = Vec::new();
        let mut hooks = Vec::new();
        loop {
            let (name, _) = self.expect_var()?;
            let init = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            items.push((name, init));
            // PHP 8.4 property hooks `{ get ...; set ...; }` (no trailing semicolon!)
            if matches!(self.kind(), TokenKind::LBrace) {
                let end = self.parse_property_hooks(&mut hooks)?;
                return Ok(ClassMember::Property {
                    attributes,
                    visibility,
                    set_visibility,
                    is_static,
                    is_readonly,
                    ty,
                    items,
                    hooks,
                    span: start.merge(end),
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect_semi()?;
        Ok(ClassMember::Property {
            attributes,
            visibility,
            set_visibility,
            is_static,
            is_readonly,
            ty,
            items,
            hooks,
            span: start.merge(end),
        })
    }

    /// Consume PHP 8.4 `(set)` suffix immediately after `public`/`protected`/`private`.
    fn eat_set_modifier(&mut self) -> bool {
        if matches!(self.kind(), TokenKind::LParen)
            && matches!(&self.peek_at(1).kind, TokenKind::Ident(s) if s.eq_ignore_ascii_case("set"))
            && matches!(self.peek_at(2).kind, TokenKind::RParen)
        {
            self.advance();
            self.advance();
            self.advance();
            true
        } else {
            false
        }
    }

    fn parse_property_hooks(&mut self, out: &mut Vec<PropertyHook>) -> Result<Span, ParseError> {
        self.expect(&TokenKind::LBrace, "'{' starting property hooks")?;
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            let hstart = self.span();
            let _hattrs = self.parse_attributes()?;
            while matches!(self.kind(), TokenKind::KwFinal | TokenKind::KwAbstract) {
                self.advance();
            }
            let by_ref = self.eat(&TokenKind::Amp);
            let (hname, _) = self.expect_member_name()?;
            let kind = match hname.to_ascii_lowercase().as_str() {
                "get" => PropHookKind::Get,
                "set" => PropHookKind::Set,
                _ => return Err(self.error_here("expected 'get' or 'set' property hook")),
            };
            let param = if matches!(self.kind(), TokenKind::LParen) {
                let ps = self.parse_params(false)?;
                ps.into_iter().next()
            } else {
                None
            };
            let body = if self.eat(&TokenKind::Semi) {
                PropHookBody::Abstract
            } else if self.eat(&TokenKind::FatArrow) {
                let e = self.parse_expr()?;
                self.expect_semi()?;
                PropHookBody::Expr(e)
            } else {
                self.expect(&TokenKind::LBrace, "'{' starting property hook body")?;
                let mut stmts = Vec::new();
                while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                    stmts.push(self.parse_stmt()?);
                }
                self.expect(&TokenKind::RBrace, "'}' closing property hook body")?;
                PropHookBody::Block(stmts)
            };
            out.push(PropertyHook {
                kind,
                by_ref,
                param,
                body,
                span: hstart.merge(self.prev_span()),
            });
        }
        self.expect(&TokenKind::RBrace, "'}' closing property hooks")
    }
}
