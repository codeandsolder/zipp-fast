//! Core recursive-descent parser machinery for `zipp-rsparse`.

mod expr;
mod item;
mod stmt;

use crate::ast::*;
use crate::token::{Span, Token, TokenKind};
use crate::{Limits, ParseError};

pub struct Parser<'a> {
    pub(crate) source: &'a str,
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
    pub(crate) depth: usize,
    pub(crate) limits: Limits,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, tokens: Vec<Token>, limits: Limits) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            depth: 0,
            limits,
        }
    }

    pub fn parse_crate(&mut self) -> Result<Crate, ParseError> {
        let mut inner_attrs = Vec::new();
        while self.is_inner_attr() {
            inner_attrs.push(self.parse_attr(true)?);
        }
        let mut items = Vec::new();
        while !self.at_eof() {
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            items.push(self.parse_item()?);
        }
        Ok(Crate { inner_attrs, items })
    }

    pub fn parse_stmt_list_until_eof(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !self.at_eof() {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // ---- Cursor helpers ----

    #[inline]
    pub(crate) fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("EOF token"))
    }

    #[inline]
    pub(crate) fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    #[inline]
    pub(crate) fn peek_at(&self, offset: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + offset)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    #[inline]
    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    #[inline]
    pub(crate) fn at(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    #[inline]
    pub(crate) fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    #[inline]
    pub(crate) fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, ParseError> {
        if self.at(kind) {
            Ok(self.bump())
        } else {
            Err(self.error(format!("expected {what}, found {:?}", self.peek_kind())))
        }
    }

    /// Consume a closing `>` in generic angle brackets, splitting `>>` (`Shr`)
    /// or `>>=` (`ShrEq`) or `>=` (`Ge`) if needed.
    pub(crate) fn eat_gt(&mut self) -> bool {
        match self.peek_kind() {
            TokenKind::Gt => {
                self.bump();
                true
            }
            TokenKind::Shr => {
                // Split `>>` into one consumed `>` and one remaining `>` in place.
                if let Some(tok) = self.tokens.get_mut(self.pos) {
                    tok.kind = TokenKind::Gt;
                    tok.span.start = tok.span.start.saturating_add(1);
                }
                true
            }
            TokenKind::Ge => {
                if let Some(tok) = self.tokens.get_mut(self.pos) {
                    tok.kind = TokenKind::Eq;
                    tok.span.start = tok.span.start.saturating_add(1);
                }
                true
            }
            TokenKind::ShrEq => {
                if let Some(tok) = self.tokens.get_mut(self.pos) {
                    tok.kind = TokenKind::Ge;
                    tok.span.start = tok.span.start.saturating_add(1);
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn expect_gt(&mut self) -> Result<(), ParseError> {
        if self.eat_gt() {
            Ok(())
        } else {
            Err(self.error(format!("expected `>`, found {:?}", self.peek_kind())))
        }
    }

    pub(crate) fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError::new(msg, self.peek().span)
    }

    pub(crate) fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            Err(self.error("maximum parser nesting depth exceeded"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(crate) fn parse_ident(&mut self) -> Result<(String, Span), ParseError> {
        match self.peek_kind() {
            TokenKind::Ident(_) => {
                let tok = self.bump();
                let TokenKind::Ident(s) = tok.kind else {
                    unreachable!()
                };
                Ok((s, tok.span))
            }
            _ => Err(self.error(format!("expected identifier, found {:?}", self.peek_kind()))),
        }
    }

    /// Parse an identifier or path keyword (`self`, `Self`, `super`, `crate`).
    pub(crate) fn parse_path_ident(&mut self) -> Result<(String, Span), ParseError> {
        let tok = self.peek().clone();
        let name = match &tok.kind {
            TokenKind::Ident(s) => s.clone(),
            TokenKind::SelfLower => "self".to_owned(),
            TokenKind::SelfUpper => "Self".to_owned(),
            TokenKind::Super => "super".to_owned(),
            TokenKind::Crate => "crate".to_owned(),
            _ => {
                return Err(self.error(format!(
                    "expected path identifier, found {:?}",
                    tok.kind
                )))
            }
        };
        self.bump();
        Ok((name, tok.span))
    }

    // ---- Attributes ----

    pub(crate) fn is_inner_attr(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Pound)
            && matches!(self.peek_at(1), TokenKind::Bang)
            && matches!(self.peek_at(2), TokenKind::LBracket)
    }

    pub(crate) fn is_outer_attr(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Pound)
            && matches!(self.peek_at(1), TokenKind::LBracket)
    }

    pub(crate) fn parse_outer_attrs(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut attrs = Vec::new();
        while self.is_outer_attr() {
            attrs.push(self.parse_attr(false)?);
        }
        Ok(attrs)
    }

    pub(crate) fn parse_attr(&mut self, is_inner: bool) -> Result<Attribute, ParseError> {
        let start = self.expect(&TokenKind::Pound, "`#`")?.span;
        if is_inner {
            self.expect(&TokenKind::Bang, "`!`")?;
        }
        self.expect(&TokenKind::LBracket, "`[`")?;
        let mut path = Vec::new();
        // Optional `unsafe(...)` in Rust 2024 attrs, or plain path
        if self.eat(&TokenKind::ColonColon) {}
        let (first, _) = self.parse_attr_ident()?;
        path.push(first);
        while self.eat(&TokenKind::ColonColon) {
            let (seg, _) = self.parse_attr_ident()?;
            path.push(seg);
        }
        let args = if self.at(&TokenKind::LParen)
            || self.at(&TokenKind::LBracket)
            || self.at(&TokenKind::LBrace)
        {
            let (_, toks) = self.parse_delimited_tokens()?;
            AttrArgs::Delimited(toks)
        } else if self.eat(&TokenKind::Eq) {
            let e = self.parse_expr()?;
            AttrArgs::Eq(Box::new(e))
        } else {
            AttrArgs::Empty
        };
        let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
        Ok(Attribute {
            is_inner,
            path,
            args,
            span: start.merge(end),
        })
    }

    fn parse_attr_ident(&mut self) -> Result<(String, Span), ParseError> {
        let tok = self.peek().clone();
        let name = match &tok.kind {
            TokenKind::Ident(s) => s.clone(),
            TokenKind::Unsafe => "unsafe".to_owned(),
            TokenKind::Crate => "crate".to_owned(),
            TokenKind::Type => "type".to_owned(),
            TokenKind::Async => "async".to_owned(),
            TokenKind::Const => "const".to_owned(),
            _ => {
                // Allow keywords in attribute paths (e.g. `#[inline]`, `#[cfg]`, `#[test]`, `#[loop]`).
                let text = self.source
                    .get(tok.span.start as usize..tok.span.end as usize)
                    .unwrap_or("");
                if !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    text.to_owned()
                } else {
                    return Err(self.error(format!("expected attribute name, found {:?}", tok.kind)));
                }
            }
        };
        self.bump();
        Ok((name, tok.span))
    }

    // ---- Visibility ----

    pub(crate) fn parse_visibility(&mut self) -> Result<Visibility, ParseError> {
        if !self.eat(&TokenKind::Pub) {
            return Ok(Visibility::Private);
        }
        if self.eat(&TokenKind::LParen) {
            let vis = if self.eat(&TokenKind::Crate) {
                Visibility::PubCrate
            } else if self.eat(&TokenKind::Super) {
                Visibility::PubSuper
            } else if self.eat(&TokenKind::SelfLower) {
                Visibility::Private
            } else {
                // `pub(in path::to::mod)`
                self.eat(&TokenKind::In);
                let _ = self.parse_path(false)?;
                Visibility::PubCrate
            };
            self.expect(&TokenKind::RParen, "`)`")?;
            Ok(vis)
        } else {
            Ok(Visibility::Public)
        }
    }

    // ---- Generics & Where Clauses ----

    pub(crate) fn skip_generics(&mut self) -> Result<(), ParseError> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(());
        }
        let mut depth = 1usize;
        while depth > 0 && !self.at_eof() {
            match self.peek_kind() {
                TokenKind::Lt => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::Gt | TokenKind::Shr | TokenKind::Ge | TokenKind::ShrEq => {
                    self.eat_gt();
                    depth -= 1;
                }
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    let _ = self.parse_delimited_tokens()?;
                }
                _ => {
                    self.bump();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn skip_where_clause(&mut self) -> Result<(), ParseError> {
        if !self.eat(&TokenKind::Where) {
            return Ok(());
        }
        let mut angle = 0usize;
        while !self.at_eof() {
            if angle == 0 && matches!(self.peek_kind(), TokenKind::LBrace | TokenKind::Semi) {
                break;
            }
            match self.peek_kind() {
                TokenKind::Lt => {
                    angle += 1;
                    self.bump();
                }
                TokenKind::Gt | TokenKind::Shr | TokenKind::Ge | TokenKind::ShrEq if angle > 0 => {
                    self.eat_gt();
                    angle -= 1;
                }
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    let _ = self.parse_delimited_tokens()?;
                }
                _ => {
                    self.bump();
                }
            }
        }
        Ok(())
    }

    // ---- Paths ----

    /// Parse a Rust path. When `in_type` is true, generic args may appear directly as `Foo<T>`;
    /// when `in_type` is false (expression/pattern), generic args require turbofish `Foo::<T>`.
    pub(crate) fn parse_path(&mut self, in_type: bool) -> Result<Path, ParseError> {
        let start = self.peek().span;
        let leading_colon = self.eat(&TokenKind::ColonColon);
        let mut segments = Vec::new();
        loop {
            let (ident, seg_span) = self.parse_path_ident()?;
            let mut type_args = Vec::new();
            if in_type && self.at(&TokenKind::Lt) {
                type_args = self.parse_generic_args()?;
            } else if in_type && self.at(&TokenKind::LParen) && matches!(ident.as_str(), "Fn" | "FnMut" | "FnOnce") {
                // `Fn(A, B) -> C`
                let _ = self.parse_delimited_tokens()?;
                if self.eat(&TokenKind::Arrow) {
                    let _ = self.parse_type()?;
                }
            } else if !in_type
                && self.at(&TokenKind::ColonColon)
                && matches!(self.peek_at(1), TokenKind::Lt)
            {
                self.bump(); // consume `::`
                type_args = self.parse_generic_args()?;
            }
            let _ = seg_span;
            segments.push(PathSegment { ident, type_args });

            if self.at(&TokenKind::ColonColon)
                && matches!(
                    self.peek_at(1),
                    TokenKind::Ident(_)
                        | TokenKind::SelfLower
                        | TokenKind::SelfUpper
                        | TokenKind::Super
                        | TokenKind::Crate
                )
            {
                self.bump();
            } else {
                break;
            }
        }
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        Ok(Path {
            leading_colon,
            segments,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_generic_args(&mut self) -> Result<Vec<Type>, ParseError> {
        self.expect(&TokenKind::Lt, "`<`")?;
        let mut args = Vec::new();
        while !self.at_eof() {
            if self.eat_gt() {
                return Ok(args);
            }
            // Skip lifetimes (`'a`)
            if matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
                self.bump();
                let _ = self.eat(&TokenKind::Comma);
                continue;
            }
            // Associated type binding `Item = T` or `Output = T`
            if matches!(self.peek_kind(), TokenKind::Ident(_))
                && matches!(self.peek_at(1), TokenKind::Eq)
            {
                self.bump();
                self.bump();
                let ty = self.parse_type()?;
                args.push(ty);
                let _ = self.eat(&TokenKind::Comma);
                continue;
            }
            // Const generic literal or block `{ N }`
            if matches!(
                self.peek_kind(),
                TokenKind::Int(_, _) | TokenKind::True | TokenKind::False | TokenKind::LBrace | TokenKind::Minus
            ) {
                let _ = self.parse_expr_no_struct()?;
                let _ = self.eat(&TokenKind::Comma);
                continue;
            }
            args.push(self.parse_type()?);
            if !self.eat(&TokenKind::Comma) {
                self.expect_gt()?;
                return Ok(args);
            }
        }
        Err(self.error("unterminated generic argument list"))
    }

    // ---- Types ----

    pub(crate) fn parse_type(&mut self) -> Result<Type, ParseError> {
        self.enter()?;
        let res = self.parse_type_inner();
        self.leave();
        res
    }

    fn parse_type_inner(&mut self) -> Result<Type, ParseError> {
        let start = self.peek().span;
        // Skip higher-ranked trait bounds `for<'a> ...`
        if self.eat(&TokenKind::For) {
            self.skip_generics()?;
        }
        let kind = match self.peek_kind() {
            TokenKind::Bang => {
                self.bump();
                TypeKind::Never
            }
            TokenKind::Ident(s) if s == "_" => {
                self.bump();
                TypeKind::Infer
            }
            TokenKind::Amp => {
                self.bump();
                if matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
                    self.bump();
                }
                let is_mut = self.eat(&TokenKind::Mut);
                let inner = self.parse_type()?;
                TypeKind::Ref {
                    is_mut,
                    inner: Box::new(inner),
                }
            }
            TokenKind::AmpAmp => {
                // `&&T`
                self.bump();
                if matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
                    self.bump();
                }
                let is_mut = self.eat(&TokenKind::Mut);
                let inner = self.parse_type()?;
                let inner_span = inner.span;
                TypeKind::Ref {
                    is_mut: false,
                    inner: Box::new(Type {
                        kind: TypeKind::Ref {
                            is_mut,
                            inner: Box::new(inner),
                        },
                        span: start.merge(inner_span),
                    }),
                }
            }
            TokenKind::Star => {
                self.bump();
                let is_mut = if self.eat(&TokenKind::Mut) {
                    true
                } else {
                    self.expect(&TokenKind::Const, "`const` or `mut`")?;
                    false
                };
                let inner = self.parse_type()?;
                TypeKind::Ptr {
                    is_mut,
                    inner: Box::new(inner),
                }
            }
            TokenKind::LParen => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.at_eof() {
                    elems.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "`)`")?;
                TypeKind::Tuple(elems)
            }
            TokenKind::LBracket => {
                self.bump();
                let elem = self.parse_type()?;
                let kind = if self.eat(&TokenKind::Semi) {
                    let len = self.parse_expr()?;
                    TypeKind::Array(Box::new(elem), Some(Box::new(len)))
                } else {
                    TypeKind::Slice(Box::new(elem))
                };
                self.expect(&TokenKind::RBracket, "`]`")?;
                kind
            }
            TokenKind::Unsafe | TokenKind::Extern | TokenKind::Fn => {
                let _ = self.eat(&TokenKind::Unsafe);
                if self.eat(&TokenKind::Extern) {
                    if matches!(self.peek_kind(), TokenKind::Str(_)) {
                        self.bump();
                    }
                }
                self.expect(&TokenKind::Fn, "`fn`")?;
                self.expect(&TokenKind::LParen, "`(`")?;
                let mut params = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.at_eof() {
                    // Optional parameter name `x: Type`
                    if matches!(self.peek_kind(), TokenKind::Ident(_))
                        && matches!(self.peek_at(1), TokenKind::Colon)
                    {
                        self.bump();
                        self.bump();
                    }
                    params.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "`)`")?;
                let ret = if self.eat(&TokenKind::Arrow) {
                    Some(Box::new(self.parse_type()?))
                } else {
                    None
                };
                TypeKind::FnPtr { params, ret }
            }
            TokenKind::Impl => {
                self.bump();
                let bounds = self.parse_trait_bounds()?;
                TypeKind::ImplTrait(bounds)
            }
            TokenKind::Dyn => {
                self.bump();
                let bounds = self.parse_trait_bounds()?;
                TypeKind::DynTrait(bounds)
            }
            TokenKind::Lt => {
                // Qualified type `<T as Trait>::Item`
                self.bump();
                let _base = self.parse_type()?;
                if self.eat(&TokenKind::As) {
                    let _ = self.parse_path(true)?;
                }
                self.expect_gt()?;
                self.expect(&TokenKind::ColonColon, "`::`")?;
                let p = self.parse_path(true)?;
                TypeKind::Path(p)
            }
            _ => {
                let path = self.parse_path(true)?;
                // Handle bare trait object bounds `Trait + Send + 'static`
                if self.at(&TokenKind::Plus) {
                    let mut bounds = vec![path];
                    while self.eat(&TokenKind::Plus) {
                        if matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
                            self.bump();
                        } else if self.eat(&TokenKind::Question) {
                            let _ = self.parse_path(true)?;
                        } else if self.at(&TokenKind::For) {
                            self.bump();
                            self.skip_generics()?;
                            bounds.push(self.parse_path(true)?);
                        } else if matches!(
                            self.peek_kind(),
                            TokenKind::Ident(_)
                                | TokenKind::ColonColon
                                | TokenKind::SelfUpper
                                | TokenKind::Crate
                                | TokenKind::Super
                        ) {
                            bounds.push(self.parse_path(true)?);
                        } else {
                            break;
                        }
                    }
                    TypeKind::DynTrait(bounds)
                } else {
                    TypeKind::Path(path)
                }
            }
        };
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        Ok(Type {
            kind,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_trait_bounds(&mut self) -> Result<Vec<Path>, ParseError> {
        let mut bounds = Vec::new();
        loop {
            if matches!(self.peek_kind(), TokenKind::Lifetime(_)) {
                self.bump();
            } else if self.eat(&TokenKind::Question) {
                let _ = self.parse_path(true)?;
            } else {
                if self.eat(&TokenKind::For) {
                    self.skip_generics()?;
                }
                bounds.push(self.parse_path(true)?);
            }
            if !self.eat(&TokenKind::Plus) {
                break;
            }
            // Trailing `+` before `,`, `>`, `{`, `)`, `;`
            if matches!(
                self.peek_kind(),
                TokenKind::Comma
                    | TokenKind::Gt
                    | TokenKind::Shr
                    | TokenKind::LBrace
                    | TokenKind::RParen
                    | TokenKind::Semi
                    | TokenKind::Eq
            ) {
                break;
            }
        }
        Ok(bounds)
    }

    // ---- Balanced token trees (macros & attributes) ----

    pub(crate) fn parse_delimited_tokens(&mut self) -> Result<(MacroDelim, Vec<Token>), ParseError> {
        let (delim, open, close) = match self.peek_kind() {
            TokenKind::LParen => (MacroDelim::Paren, TokenKind::LParen, TokenKind::RParen),
            TokenKind::LBracket => (MacroDelim::Bracket, TokenKind::LBracket, TokenKind::RBracket),
            TokenKind::LBrace => (MacroDelim::Brace, TokenKind::LBrace, TokenKind::RBrace),
            _ => {
                return Err(self.error(format!(
                    "expected `(`, `[`, or `{{`, found {:?}",
                    self.peek_kind()
                )))
            }
        };
        self.bump();
        let mut out = Vec::new();
        let mut depth = 1usize;
        while depth > 0 {
            if self.at_eof() {
                return Err(self.error("unterminated delimited token tree"));
            }
            let tok = self.bump();
            if tok.kind == open {
                depth += 1;
                out.push(tok);
            } else if tok.kind == close {
                depth -= 1;
                if depth > 0 {
                    out.push(tok);
                }
            } else {
                out.push(tok);
            }
        }
        Ok((delim, out))
    }
}
