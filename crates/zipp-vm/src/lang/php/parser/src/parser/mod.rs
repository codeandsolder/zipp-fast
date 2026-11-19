//! PHP 8.x recursive-descent + Pratt parser.

mod decl;
mod expr;
mod stmt;

use crate::ast::*;
use crate::token::{Span, Token, TokenKind};
use crate::{Limits, ParseError};

pub fn parse_tokens(source: &str, tokens: &[Token], limits: Limits) -> Result<Program, ParseError> {
    let mut p = Parser {
        source,
        tokens,
        pos: 0,
        depth: 0,
        limits,
    };
    let mut stmts = Vec::new();
    while !p.is_eof() {
        stmts.push(p.parse_stmt()?);
    }
    Ok(Program { stmts })
}

pub(crate) struct Parser<'a> {
    pub(crate) source: &'a str,
    pub(crate) tokens: &'a [Token],
    pub(crate) pos: usize,
    pub(crate) depth: usize,
    pub(crate) limits: Limits,
}

impl<'a> Parser<'a> {
    pub(crate) fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            Err(self.error_here("maximum parser nesting depth exceeded"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    #[inline]
    pub(crate) fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    #[inline]
    pub(crate) fn peek_at(&self, offset: usize) -> &Token {
        &self.tokens[(self.pos + offset).min(self.tokens.len() - 1)]
    }

    #[inline]
    pub(crate) fn kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    #[inline]
    pub(crate) fn is_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    #[inline]
    pub(crate) fn span(&self) -> Span {
        self.peek().span
    }

    #[inline]
    pub(crate) fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    pub(crate) fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn eat(&mut self, expected: &TokenKind) -> bool {
        if self.kind() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, expected: &TokenKind, what: &str) -> Result<Span, ParseError> {
        if self.kind() == expected {
            Ok(self.advance().span)
        } else {
            Err(self.error_here(&format!("expected {what}, got {:?}", self.kind())))
        }
    }

    /// Consume `;` or accept `?>` as an implicit statement terminator.
    pub(crate) fn expect_semi(&mut self) -> Result<Span, ParseError> {
        match self.kind() {
            TokenKind::Semi => Ok(self.advance().span),
            TokenKind::CloseTag => Ok(self.advance().span),
            TokenKind::Eof => Ok(self.span()),
            _ => Err(self.error_here(&format!("expected ';', got {:?}", self.kind()))),
        }
    }

    pub(crate) fn error_here(&self, msg: &str) -> ParseError {
        ParseError::new(self.source, self.span().start, msg)
    }

    pub(crate) fn error_at(&self, span: Span, msg: &str) -> ParseError {
        ParseError::new(self.source, span.start, msg)
    }

    pub(crate) fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
        match self.kind().clone() {
            TokenKind::Ident(s) => {
                let sp = self.advance().span;
                Ok((s, sp))
            }
            _ => Err(self.error_here(&format!("expected identifier, got {:?}", self.kind()))),
        }
    }

    /// Identifier or semi-reserved keyword (allowed for methods, properties, constants, names).
    pub(crate) fn expect_member_name(&mut self) -> Result<(String, Span), ParseError> {
        if let Some(s) = self.kind().as_member_name() {
            let owned = s.to_owned();
            let sp = self.advance().span;
            Ok((owned, sp))
        } else {
            Err(self.error_here(&format!("expected member name, got {:?}", self.kind())))
        }
    }

    pub(crate) fn expect_var(&mut self) -> Result<(String, Span), ParseError> {
        match self.kind().clone() {
            TokenKind::Var(s) => {
                let sp = self.advance().span;
                Ok((s, sp))
            }
            _ => Err(self.error_here(&format!("expected variable, got {:?}", self.kind()))),
        }
    }

    /// Parse a PHP qualified / fully-qualified / relative name (`\A\B`, `namespace\A`, `A\B`, `A`).
    pub(crate) fn parse_name(&mut self) -> Result<Name, ParseError> {
        let start = self.span();
        let kind = if self.eat(&TokenKind::Backslash) {
            NameKind::FullyQualified
        } else if matches!(self.kind(), TokenKind::KwNamespace)
            && matches!(self.peek_at(1).kind, TokenKind::Backslash)
        {
            self.advance(); // namespace
            self.advance(); // \
            NameKind::Relative
        } else {
            NameKind::Unqualified
        };
        let mut parts = Vec::new();
        let (first, _) = self.expect_member_name()?;
        parts.push(first);
        while self.eat(&TokenKind::Backslash) {
            let (seg, _) = self.expect_member_name()?;
            parts.push(seg);
        }
        let span = start.merge(self.prev_span());
        Ok(Name { kind, parts, span })
    }

    /// Parse PHP 8 attributes `#[Attr1, Attr2(1, a: 2)]` (zero or more groups).
    pub(crate) fn parse_attributes(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut out = Vec::new();
        while self.eat(&TokenKind::AttrOpen) {
            while !matches!(self.kind(), TokenKind::RBracket | TokenKind::Eof) {
                let start = self.span();
                let name = self.parse_name()?;
                let args = if matches!(self.kind(), TokenKind::LParen) {
                    match self.parse_call_args()? {
                        CallArgs::Normal(a) => a,
                        CallArgs::FirstClassCallable => {
                            return Err(self.error_at(start, "first-class callable not allowed in attribute"));
                        }
                    }
                } else {
                    Vec::new()
                };
                out.push(Attribute {
                    name,
                    args,
                    span: start.merge(self.prev_span()),
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBracket, "']' closing attribute")?;
        }
        Ok(out)
    }

    /// Check whether the current position can start a type hint.
    pub(crate) fn starts_type(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Question
                | TokenKind::Backslash
                | TokenKind::Ident(_)
                | TokenKind::KwArray
                | TokenKind::KwCallable
                | TokenKind::KwStatic
                | TokenKind::KwNamespace
                | TokenKind::KwNull
                | TokenKind::KwFalse
                | TokenKind::KwTrue
                | TokenKind::LParen
        )
    }

    /// Parse a PHP 8 type hint (`?int`, `int|string|null`, `A&B`, `(A&B)|null`).
    pub(crate) fn parse_type(&mut self) -> Result<TypeHint, ParseError> {
        if self.eat(&TokenKind::Question) {
            let inner = self.parse_single_type()?;
            return Ok(TypeHint::Nullable(Box::new(inner)));
        }
        let first = self.parse_intersect_or_single_type()?;
        if matches!(self.kind(), TokenKind::Pipe) {
            let mut members = vec![first];
            while self.eat(&TokenKind::Pipe) {
                members.push(self.parse_intersect_or_single_type()?);
            }
            return Ok(TypeHint::Union(members));
        }
        Ok(first)
    }

    fn parse_intersect_or_single_type(&mut self) -> Result<TypeHint, ParseError> {
        if self.eat(&TokenKind::LParen) {
            let first = self.parse_single_type()?;
            let mut items = vec![first];
            while self.eat(&TokenKind::Amp) {
                items.push(self.parse_single_type()?);
            }
            self.expect(&TokenKind::RParen, "')' closing DNF type")?;
            return Ok(if items.len() == 1 {
                items.pop().unwrap()
            } else {
                TypeHint::Intersection(items)
            });
        }
        let first = self.parse_single_type()?;
        // Disambiguate intersection `A&B` vs by-ref parameter `Foo &$x`:
        // Only treat `&` as intersection when NOT followed by `$var` or `...`!
        if matches!(self.kind(), TokenKind::Amp)
            && !matches!(self.peek_at(1).kind, TokenKind::Var(_) | TokenKind::Ellipsis)
        {
            let mut items = vec![first];
            while matches!(self.kind(), TokenKind::Amp)
                && !matches!(self.peek_at(1).kind, TokenKind::Var(_) | TokenKind::Ellipsis)
            {
                self.advance();
                items.push(self.parse_single_type()?);
            }
            return Ok(TypeHint::Intersection(items));
        }
        Ok(first)
    }

    fn parse_single_type(&mut self) -> Result<TypeHint, ParseError> {
        let name = self.parse_name()?;
        Ok(TypeHint::Simple(name))
    }
}
