//! Block, statement, and pattern parsing for `zipp-rsparse`.

use super::Parser;
use crate::ast::*;
use crate::token::TokenKind;
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn parse_block(&mut self) -> Result<Block, ParseError> {
        self.enter()?;
        let res = self.parse_block_inner();
        self.leave();
        res
    }

    fn parse_block_inner(&mut self) -> Result<Block, ParseError> {
        let start = self.expect(&TokenKind::LBrace, "`{`")?.span;
        while self.is_inner_attr() {
            let _ = self.parse_attr(true)?;
        }
        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            if self.at(&TokenKind::Let) {
                stmts.push(self.parse_let_stmt()?);
                continue;
            }
            if self.can_start_item() {
                let item = self.parse_item()?;
                let span = item.span;
                stmts.push(Stmt {
                    kind: StmtKind::Item(Box::new(item)),
                    span,
                });
                continue;
            }
            let expr = self.parse_stmt_expr()?;
            let span = expr.span;
            if self.eat(&TokenKind::Semi) {
                stmts.push(Stmt {
                    kind: StmtKind::Expr {
                        expr,
                        has_semi: true,
                    },
                    span,
                });
            } else if self.at(&TokenKind::RBrace) {
                tail = Some(Box::new(expr));
                break;
            } else if expr_needs_no_semi(&expr.kind) {
                stmts.push(Stmt {
                    kind: StmtKind::Expr {
                        expr,
                        has_semi: false,
                    },
                    span,
                });
            } else {
                return Err(self.error(format!(
                    "expected `;` or `}}` after expression, found {:?}",
                    self.peek_kind()
                )));
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Block {
            stmts,
            tail,
            span: start.merge(end),
        })
    }

    pub(crate) fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        if self.eat(&TokenKind::Semi) {
            let span = self.tokens[self.pos.saturating_sub(1)].span;
            return Ok(Stmt {
                kind: StmtKind::Empty,
                span,
            });
        }
        if self.at(&TokenKind::Let) {
            return self.parse_let_stmt();
        }
        if self.can_start_item() {
            let item = self.parse_item()?;
            let span = item.span;
            return Ok(Stmt {
                kind: StmtKind::Item(Box::new(item)),
                span,
            });
        }
        let expr = self.parse_stmt_expr()?;
        let span = expr.span;
        let has_semi = self.eat(&TokenKind::Semi);
        Ok(Stmt {
            kind: StmtKind::Expr { expr, has_semi },
            span,
        })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Let, "`let`")?.span;
        let pat = self.parse_pat()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let init = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let else_block = if self.eat(&TokenKind::Else) {
            Some(self.parse_block()?)
        } else {
            None
        };
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Let {
                pat,
                ty,
                init,
                else_block,
            },
            span: start.merge(end),
        })
    }

    // ---- Patterns ----

    pub(crate) fn parse_pat(&mut self) -> Result<Pat, ParseError> {
        self.enter()?;
        let res = self.parse_pat_or();
        self.leave();
        res
    }

    pub(crate) fn parse_pat_no_or(&mut self) -> Result<Pat, ParseError> {
        self.enter()?;
        let res = self.parse_pat_atom();
        self.leave();
        res
    }

    fn parse_pat_or(&mut self) -> Result<Pat, ParseError> {
        let _ = self.eat(&TokenKind::Pipe); // leading `|` allowed in Rust
        let first = self.parse_pat_atom()?;
        if !self.at(&TokenKind::Pipe) {
            return Ok(first);
        }
        let start = first.span;
        let mut pats = vec![first];
        while self.eat(&TokenKind::Pipe) {
            pats.push(self.parse_pat_atom()?);
        }
        let end = pats.last().unwrap().span;
        Ok(Pat {
            kind: PatKind::Or(pats),
            span: start.merge(end),
        })
    }

    fn parse_pat_atom(&mut self) -> Result<Pat, ParseError> {
        let start = self.peek().span;
        match self.peek_kind().clone() {
            TokenKind::DotDot => {
                self.bump();
                if self.can_start_range_pat_bound() {
                    let end_e = self.parse_pat_range_bound()?;
                    let span = start.merge(end_e.span);
                    return Ok(Pat {
                        kind: PatKind::Range {
                            start: None,
                            end: Some(Box::new(end_e)),
                            inclusive: false,
                        },
                        span,
                    });
                }
                Ok(Pat {
                    kind: PatKind::Rest,
                    span: start,
                })
            }
            TokenKind::DotDotEq => {
                self.bump();
                let end_e = self.parse_pat_range_bound()?;
                let span = start.merge(end_e.span);
                Ok(Pat {
                    kind: PatKind::Range {
                        start: None,
                        end: Some(Box::new(end_e)),
                        inclusive: true,
                    },
                    span,
                })
            }
            TokenKind::Amp => {
                self.bump();
                let is_mut = self.eat(&TokenKind::Mut);
                let inner = self.parse_pat_atom()?;
                let span = start.merge(inner.span);
                Ok(Pat {
                    kind: PatKind::Ref {
                        is_mut,
                        inner: Box::new(inner),
                    },
                    span,
                })
            }
            TokenKind::AmpAmp => {
                self.bump();
                let is_mut = self.eat(&TokenKind::Mut);
                let inner = self.parse_pat_atom()?;
                let span = start.merge(inner.span);
                Ok(Pat {
                    kind: PatKind::Ref {
                        is_mut: false,
                        inner: Box::new(Pat {
                            kind: PatKind::Ref {
                                is_mut,
                                inner: Box::new(inner),
                            },
                            span,
                        }),
                    },
                    span,
                })
            }
            TokenKind::LParen => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.at_eof() {
                    elems.push(self.parse_pat()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                Ok(Pat {
                    kind: PatKind::Tuple(elems),
                    span: start.merge(end),
                })
            }
            TokenKind::LBracket => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(&TokenKind::RBracket) && !self.at_eof() {
                    elems.push(self.parse_pat()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                Ok(Pat {
                    kind: PatKind::Slice(elems),
                    span: start.merge(end),
                })
            }
            TokenKind::Int(v, s) => {
                self.bump();
                self.maybe_finish_range_pat(Literal::Int(v, s), false, start)
            }
            TokenKind::Float(v, s) => {
                self.bump();
                self.maybe_finish_range_pat(Literal::Float(v, s), false, start)
            }
            TokenKind::Char(c) => {
                self.bump();
                self.maybe_finish_range_pat(Literal::Char(c), false, start)
            }
            TokenKind::Byte(b) => {
                self.bump();
                self.maybe_finish_range_pat(Literal::Byte(b), false, start)
            }
            TokenKind::Str(s) => {
                self.bump();
                Ok(Pat {
                    kind: PatKind::Literal(Literal::Str(s)),
                    span: start,
                })
            }
            TokenKind::ByteStr(bs) => {
                self.bump();
                Ok(Pat {
                    kind: PatKind::Literal(Literal::ByteStr(bs)),
                    span: start,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Pat {
                    kind: PatKind::Literal(Literal::Bool(true)),
                    span: start,
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Pat {
                    kind: PatKind::Literal(Literal::Bool(false)),
                    span: start,
                })
            }
            TokenKind::Minus => {
                self.bump();
                let lit = match self.peek_kind().clone() {
                    TokenKind::Int(v, s) => {
                        self.bump();
                        Literal::Int(v, s)
                    }
                    TokenKind::Float(v, s) => {
                        self.bump();
                        Literal::Float(v, s)
                    }
                    _ => return Err(self.error("expected numeric literal after `-` in pattern")),
                };
                self.maybe_finish_range_pat(lit, true, start)
            }
            TokenKind::Ref | TokenKind::Mut => {
                let is_ref = self.eat(&TokenKind::Ref);
                let is_mut = self.eat(&TokenKind::Mut);
                let (name, id_span) = self.parse_ident()?;
                let subpat = if self.eat(&TokenKind::At) {
                    Some(Box::new(self.parse_pat_atom()?))
                } else {
                    None
                };
                let end = subpat.as_ref().map(|p| p.span).unwrap_or(id_span);
                Ok(Pat {
                    kind: PatKind::Ident {
                        is_mut,
                        is_ref,
                        name,
                        subpat,
                    },
                    span: start.merge(end),
                })
            }
            TokenKind::Ident(s) if s == "_" => {
                self.bump();
                Ok(Pat {
                    kind: PatKind::Wildcard,
                    span: start,
                })
            }
            TokenKind::Ident(_)
            | TokenKind::ColonColon
            | TokenKind::SelfUpper
            | TokenKind::Crate
            | TokenKind::Super => {
                // Single identifier binding with `@` or plain binding vs path/struct/tuple-struct
                if matches!(self.peek_kind(), TokenKind::Ident(_))
                    && !matches!(
                        self.peek_at(1),
                        TokenKind::ColonColon | TokenKind::LParen | TokenKind::LBrace | TokenKind::DotDot | TokenKind::DotDotEq
                    )
                {
                    let (name, id_span) = self.parse_ident()?;
                    if self.eat(&TokenKind::At) {
                        let sub = self.parse_pat_atom()?;
                        let span = start.merge(sub.span);
                        return Ok(Pat {
                            kind: PatKind::Ident {
                                is_mut: false,
                                is_ref: false,
                                name,
                                subpat: Some(Box::new(sub)),
                            },
                            span,
                        });
                    }
                    // Heuristic: Uppercase single ident in a pattern could be a unit enum variant
                    // (`None`, `North`, etc.) or constant (`MAX`). Store as `Path` when it starts
                    // with uppercase, `Ident` otherwise — the emitter checks enum/const scope too!
                    if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                        return Ok(Pat {
                            kind: PatKind::Path(Path::simple(name, id_span)),
                            span: id_span,
                        });
                    }
                    return Ok(Pat {
                        kind: PatKind::Ident {
                            is_mut: false,
                            is_ref: false,
                            name,
                            subpat: None,
                        },
                        span: id_span,
                    });
                }

                let path = self.parse_path(false)?;
                if self.eat(&TokenKind::LParen) {
                    let mut elems = Vec::new();
                    while !self.at(&TokenKind::RParen) && !self.at_eof() {
                        elems.push(self.parse_pat()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                    Ok(Pat {
                        kind: PatKind::TupleStruct { path, elems },
                        span: start.merge(end),
                    })
                } else if self.eat(&TokenKind::LBrace) {
                    let mut fields = Vec::new();
                    let mut has_rest = false;
                    while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                        if self.eat(&TokenKind::DotDot) {
                            has_rest = true;
                            let _ = self.eat(&TokenKind::Comma);
                            break;
                        }
                        let fstart = self.peek().span;
                        let is_ref = self.eat(&TokenKind::Ref);
                        let is_mut = self.eat(&TokenKind::Mut);
                        let (fname, fspan) = self.parse_ident()?;
                        let pat = if !is_ref && !is_mut && self.eat(&TokenKind::Colon) {
                            self.parse_pat()?
                        } else {
                            Pat {
                                kind: PatKind::Ident {
                                    is_mut,
                                    is_ref,
                                    name: fname.clone(),
                                    subpat: None,
                                },
                                span: fstart.merge(fspan),
                            }
                        };
                        let fend = pat.span;
                        fields.push(FieldPat {
                            name: fname,
                            pat,
                            span: fstart.merge(fend),
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
                    Ok(Pat {
                        kind: PatKind::Struct {
                            path,
                            fields,
                            has_rest,
                        },
                        span: start.merge(end),
                    })
                } else if self.at(&TokenKind::DotDot) || self.at(&TokenKind::DotDotEq) {
                    let inclusive = self.eat(&TokenKind::DotDotEq);
                    if !inclusive {
                        self.bump();
                    }
                    let end_e = if self.can_start_range_pat_bound() {
                        Some(Box::new(self.parse_pat_range_bound()?))
                    } else {
                        None
                    };
                    let end_span = end_e
                        .as_ref()
                        .map(|e| e.span)
                        .unwrap_or_else(|| self.tokens[self.pos.saturating_sub(1)].span);
                    let pspan = path.span;
                    Ok(Pat {
                        kind: PatKind::Range {
                            start: Some(Box::new(Expr {
                                kind: ExprKind::Path(path),
                                span: pspan,
                            })),
                            end: end_e,
                            inclusive,
                        },
                        span: start.merge(end_span),
                    })
                } else {
                    let span = path.span;
                    Ok(Pat {
                        kind: PatKind::Path(path),
                        span,
                    })
                }
            }
            _ => Err(self.error(format!("expected pattern, found {:?}", self.peek_kind()))),
        }
    }

    fn maybe_finish_range_pat(
        &mut self,
        lit: Literal,
        neg: bool,
        start: crate::token::Span,
    ) -> Result<Pat, ParseError> {
        let end_lit = self.tokens[self.pos.saturating_sub(1)].span;
        if self.at(&TokenKind::DotDot)
            || self.at(&TokenKind::DotDotEq)
            || self.at(&TokenKind::DotDotDot)
        {
            let inclusive = self.eat(&TokenKind::DotDotEq) || self.eat(&TokenKind::DotDotDot);
            if !inclusive {
                self.bump();
            }
            let start_expr = {
                let base = Expr {
                    kind: ExprKind::Literal(lit),
                    span: start.merge(end_lit),
                };
                if neg {
                    Expr {
                        kind: ExprKind::Unary {
                            op: UnOp::Neg,
                            expr: Box::new(base),
                        },
                        span: start.merge(end_lit),
                    }
                } else {
                    base
                }
            };
            let end_expr = if self.can_start_range_pat_bound() {
                Some(Box::new(self.parse_pat_range_bound()?))
            } else {
                None
            };
            let end = end_expr
                .as_ref()
                .map(|e| e.span)
                .unwrap_or_else(|| self.tokens[self.pos.saturating_sub(1)].span);
            return Ok(Pat {
                kind: PatKind::Range {
                    start: Some(Box::new(start_expr)),
                    end: end_expr,
                    inclusive,
                },
                span: start.merge(end),
            });
        }
        Ok(Pat {
            kind: if neg {
                PatKind::NegLiteral(lit)
            } else {
                PatKind::Literal(lit)
            },
            span: start.merge(end_lit),
        })
    }

    fn can_start_range_pat_bound(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Int(_, _)
                | TokenKind::Float(_, _)
                | TokenKind::Char(_)
                | TokenKind::Byte(_)
                | TokenKind::Minus
                | TokenKind::Ident(_)
                | TokenKind::ColonColon
        )
    }

    fn parse_pat_range_bound(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek().span;
        match self.peek_kind().clone() {
            TokenKind::Minus => {
                self.bump();
                let inner = self.parse_pat_range_bound()?;
                let span = start.merge(inner.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(inner),
                    },
                    span,
                })
            }
            TokenKind::Int(v, s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Int(v, s)),
                    span: start,
                })
            }
            TokenKind::Float(v, s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Float(v, s)),
                    span: start,
                })
            }
            TokenKind::Char(c) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Char(c)),
                    span: start,
                })
            }
            TokenKind::Byte(b) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Byte(b)),
                    span: start,
                })
            }
            _ => {
                let path = self.parse_path(false)?;
                let span = path.span;
                Ok(Expr {
                    kind: ExprKind::Path(path),
                    span,
                })
            }
        }
    }
}

pub(crate) fn expr_needs_no_semi(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Block(_)
            | ExprKind::UnsafeBlock(_)
            | ExprKind::AsyncBlock { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Loop { .. }
            | ExprKind::While { .. }
            | ExprKind::For { .. }
            | ExprKind::MacroCall(MacroCall {
                delim: MacroDelim::Brace,
                ..
            })
    )
}
