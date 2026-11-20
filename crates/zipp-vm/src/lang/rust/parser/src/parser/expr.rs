//! Pratt expression parser for `zipp-rsparse`.

use super::Parser;
use crate::ast::*;
use crate::token::{Span, TokenKind};
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_bp(0, true)
    }

    pub(crate) fn parse_expr_no_struct(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_bp(0, false)
    }

    /// Parse an expression in statement position: per Rust's grammar, a block-like
    /// expression (`if`, `match`, `loop`, `while`, `for`, `{ ... }`, `async { ... }`)
    /// terminates immediately unless followed by `.` or `?`, so a following `*x = 1`,
    /// `-1`, `(a, b)`, or `[1, 2]` starts the next statement instead of binding as an operator.
    pub(crate) fn parse_stmt_expr(&mut self) -> Result<Expr, ParseError> {
        self.enter()?;
        let res = (|| {
            let lhs = self.parse_prefix_expr(true)?;
            if super::stmt::expr_needs_no_semi(&lhs.kind)
                && !matches!(self.peek_kind(), TokenKind::Dot | TokenKind::Question)
            {
                return Ok(lhs);
            }
            self.parse_expr_rest(lhs, 0, true)
        })();
        self.leave();
        res
    }

    fn parse_expr_bp(&mut self, min_bp: u8, allow_struct: bool) -> Result<Expr, ParseError> {
        self.enter()?;
        let res = self.parse_expr_bp_inner(min_bp, allow_struct);
        self.leave();
        res
    }

    fn parse_expr_bp_inner(&mut self, min_bp: u8, allow_struct: bool) -> Result<Expr, ParseError> {
        let lhs = self.parse_prefix_expr(allow_struct)?;
        self.parse_expr_rest(lhs, min_bp, allow_struct)
    }

    fn parse_expr_rest(
        &mut self,
        mut lhs: Expr,
        min_bp: u8,
        allow_struct: bool,
    ) -> Result<Expr, ParseError> {

        loop {
            // Postfix operators (highest precedence: `.`, `()`, `[]`, `?`)
            loop {
                match self.peek_kind() {
                    TokenKind::Question => {
                        let qspan = self.bump().span;
                        let span = lhs.span.merge(qspan);
                        lhs = Expr {
                            kind: ExprKind::Try(Box::new(lhs)),
                            span,
                        };
                    }
                    TokenKind::LParen => {
                        let args = self.parse_call_args()?;
                        let end = self.tokens[self.pos.saturating_sub(1)].span;
                        let span = lhs.span.merge(end);
                        lhs = Expr {
                            kind: ExprKind::Call {
                                callee: Box::new(lhs),
                                args,
                            },
                            span,
                        };
                    }
                    TokenKind::LBracket => {
                        self.bump();
                        let index = self.parse_expr()?;
                        let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                        let span = lhs.span.merge(end);
                        lhs = Expr {
                            kind: ExprKind::Index {
                                receiver: Box::new(lhs),
                                index: Box::new(index),
                            },
                            span,
                        };
                    }
                    TokenKind::Dot => {
                        self.bump();
                        lhs = self.parse_dot_postfix(lhs)?;
                    }
                    _ => break,
                }
            }

            // `as Type` cast (precedence 22)
            if self.at(&TokenKind::As) {
                if 22 < min_bp {
                    break;
                }
                self.bump();
                let ty = self.parse_type()?;
                let span = lhs.span.merge(ty.span);
                lhs = Expr {
                    kind: ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    },
                    span,
                };
                continue;
            }

            // Range operators `..` and `..=` (precedence 4, non-associative)
            if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq) {
                if 4 < min_bp {
                    break;
                }
                let inclusive = self.eat(&TokenKind::DotDotEq);
                let op_span = if inclusive {
                    self.tokens[self.pos.saturating_sub(1)].span
                } else {
                    self.bump().span
                };
                let end_expr = if self.can_start_expr() {
                    Some(Box::new(self.parse_expr_bp(5, allow_struct)?))
                } else {
                    None
                };
                let span = lhs.span.merge(
                    end_expr.as_ref().map(|e| e.span).unwrap_or(op_span),
                );
                lhs = Expr {
                    kind: ExprKind::Range {
                        start: Some(Box::new(lhs)),
                        end: end_expr,
                        inclusive,
                    },
                    span,
                };
                continue;
            }

            // Assignment operators (precedence 2, right-associative)
            if let Some(assign_op) = self.peek_assign_op() {
                if 2 < min_bp {
                    break;
                }
                self.bump();
                let rhs = self.parse_expr_bp(2, allow_struct)?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::Assign {
                        op: assign_op,
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                    },
                    span,
                };
                continue;
            }

            // Binary operators
            if let Some((op, l_bp, r_bp)) = self.peek_bin_op() {
                if l_bp < min_bp {
                    break;
                }
                self.bump();
                let rhs = self.parse_expr_bp(r_bp, allow_struct)?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                    },
                    span,
                };
                continue;
            }

            break;
        }

        Ok(lhs)
    }

    fn parse_dot_postfix(&mut self, receiver: Expr) -> Result<Expr, ParseError> {
        let recv_span = receiver.span;
        // `.await`
        if self.eat(&TokenKind::Await) {
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            return Ok(Expr {
                kind: ExprKind::Await(Box::new(receiver)),
                span: recv_span.merge(end),
            });
        }
        // `.0` tuple index
        if let TokenKind::Int(v, _) = self.peek_kind().clone() {
            let span = self.bump().span;
            return Ok(Expr {
                kind: ExprKind::TupleIndex {
                    receiver: Box::new(receiver),
                    index: v as usize,
                },
                span: recv_span.merge(span),
            });
        }
        // Chained tuple index `.0.1` lexed as a float token `"0.1"`
        if matches!(self.peek_kind(), TokenKind::Float(_, _)) {
            let tok = self.bump();
            let raw = self
                .source
                .get(tok.span.start as usize..tok.span.end as usize)
                .unwrap_or("");
            if let Some((a, b)) = raw.split_once('.') {
                if let (Ok(i1), Ok(i2)) = (a.parse::<usize>(), b.parse::<usize>()) {
                    let mid = Expr {
                        kind: ExprKind::TupleIndex {
                            receiver: Box::new(receiver),
                            index: i1,
                        },
                        span: recv_span.merge(tok.span),
                    };
                    return Ok(Expr {
                        kind: ExprKind::TupleIndex {
                            receiver: Box::new(mid),
                            index: i2,
                        },
                        span: recv_span.merge(tok.span),
                    });
                }
            }
            return Err(ParseError::new("invalid tuple index", tok.span));
        }

        // Field access or method call (`.foo`, `.foo()`, `.foo::<T>()`)
        let (name, nspan) = self.parse_ident()?;
        let mut type_args = Vec::new();
        if self.at(&TokenKind::ColonColon) && matches!(self.peek_at(1), TokenKind::Lt) {
            self.bump();
            type_args = self.parse_generic_args()?;
        }
        if self.at(&TokenKind::LParen) {
            let args = self.parse_call_args()?;
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            Ok(Expr {
                kind: ExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: name,
                    type_args,
                    args,
                },
                span: recv_span.merge(end),
            })
        } else {
            Ok(Expr {
                kind: ExprKind::Field {
                    receiver: Box::new(receiver),
                    field: name,
                },
                span: recv_span.merge(nspan),
            })
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        Ok(args)
    }

    fn parse_prefix_expr(&mut self, allow_struct: bool) -> Result<Expr, ParseError> {
        let start = self.peek().span;
        match self.peek_kind().clone() {
            // Literals
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
            TokenKind::Str(s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Str(s)),
                    span: start,
                })
            }
            TokenKind::ByteStr(bs) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::ByteStr(bs)),
                    span: start,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Bool(true)),
                    span: start,
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Bool(false)),
                    span: start,
                })
            }

            // Unary operators (precedence 24)
            TokenKind::Minus => {
                self.bump();
                let e = self.parse_expr_bp(24, allow_struct)?;
                let span = start.merge(e.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(e),
                    },
                    span,
                })
            }
            TokenKind::Bang => {
                self.bump();
                let e = self.parse_expr_bp(24, allow_struct)?;
                let span = start.merge(e.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Not,
                        expr: Box::new(e),
                    },
                    span,
                })
            }
            TokenKind::Star => {
                self.bump();
                let e = self.parse_expr_bp(24, allow_struct)?;
                let span = start.merge(e.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Deref,
                        expr: Box::new(e),
                    },
                    span,
                })
            }
            TokenKind::Amp => {
                self.bump();
                let is_mut = self.eat(&TokenKind::Mut);
                if !is_mut {
                    // Handle `&raw const` / `&raw mut` (only when followed by `const` or `mut`!)
                    if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "raw")
                        && matches!(self.peek_at(1), TokenKind::Const | TokenKind::Mut)
                    {
                        self.bump();
                        self.bump();
                    }
                }
                let e = self.parse_expr_bp(24, allow_struct)?;
                let span = start.merge(e.span);
                Ok(Expr {
                    kind: ExprKind::Ref {
                        is_mut,
                        expr: Box::new(e),
                    },
                    span,
                })
            }
            TokenKind::AmpAmp => {
                // `&&expr` -> `&(&expr)`
                self.bump();
                let is_mut = self.eat(&TokenKind::Mut);
                let e = self.parse_expr_bp(24, allow_struct)?;
                let span = start.merge(e.span);
                Ok(Expr {
                    kind: ExprKind::Ref {
                        is_mut: false,
                        expr: Box::new(Expr {
                            kind: ExprKind::Ref {
                                is_mut,
                                expr: Box::new(e),
                            },
                            span,
                        }),
                    },
                    span,
                })
            }

            // Prefix range (`..`, `..b`, `..=b`)
            TokenKind::DotDot | TokenKind::DotDotEq => {
                let inclusive = self.eat(&TokenKind::DotDotEq);
                if !inclusive {
                    self.bump();
                }
                let end_expr = if self.can_start_expr() {
                    Some(Box::new(self.parse_expr_bp(5, allow_struct)?))
                } else {
                    None
                };
                let span = start.merge(end_expr.as_ref().map(|e| e.span).unwrap_or(start));
                Ok(Expr {
                    kind: ExprKind::Range {
                        start: None,
                        end: end_expr,
                        inclusive,
                    },
                    span,
                })
            }

            // Parenthesized expression or Tuple
            TokenKind::LParen => {
                self.bump();
                if self.eat(&TokenKind::RParen) {
                    let end = self.tokens[self.pos.saturating_sub(1)].span;
                    return Ok(Expr {
                        kind: ExprKind::Tuple(Vec::new()),
                        span: start.merge(end),
                    });
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while !self.at(&TokenKind::RParen) && !self.at_eof() {
                        elems.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                    Ok(Expr {
                        kind: ExprKind::Tuple(elems),
                        span: start.merge(end),
                    })
                } else {
                    let _ = self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(first)
                }
            }

            // Array literal `[a, b, c]` or repeat `[val; count]`
            TokenKind::LBracket => {
                self.bump();
                if self.eat(&TokenKind::RBracket) {
                    let end = self.tokens[self.pos.saturating_sub(1)].span;
                    return Ok(Expr {
                        kind: ExprKind::Array(Vec::new()),
                        span: start.merge(end),
                    });
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Semi) {
                    let count = self.parse_expr()?;
                    let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                    Ok(Expr {
                        kind: ExprKind::Repeat {
                            elem: Box::new(first),
                            count: Box::new(count),
                        },
                        span: start.merge(end),
                    })
                } else {
                    let mut elems = vec![first];
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::RBracket) {
                            break;
                        }
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                    Ok(Expr {
                        kind: ExprKind::Array(elems),
                        span: start.merge(end),
                    })
                }
            }

            // Block
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                let span = block.span;
                Ok(Expr {
                    kind: ExprKind::Block(block),
                    span,
                })
            }

            // `unsafe { ... }`
            TokenKind::Unsafe => {
                self.bump();
                let block = self.parse_block()?;
                let span = start.merge(block.span);
                Ok(Expr {
                    kind: ExprKind::UnsafeBlock(block),
                    span,
                })
            }

            // `async { ... }`, `async move { ... }`, `async |...| ...`, `async move |...| ...`
            TokenKind::Async => {
                self.bump();
                let is_move = self.eat(&TokenKind::Move);
                if self.at(&TokenKind::Pipe) || self.at(&TokenKind::PipePipe) {
                    self.parse_closure(true, is_move, start)
                } else {
                    let block = self.parse_block()?;
                    let span = start.merge(block.span);
                    Ok(Expr {
                        kind: ExprKind::AsyncBlock { is_move, block },
                        span,
                    })
                }
            }

            // Closures `|x| ...`, `|| ...`, `move |x| ...`
            TokenKind::Move => {
                self.bump();
                self.parse_closure(false, true, start)
            }
            TokenKind::Pipe | TokenKind::PipePipe => self.parse_closure(false, false, start),

            // Control flow
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Loop => self.parse_loop_expr(None, start),
            TokenKind::While => self.parse_while_expr(None, start),
            TokenKind::For => self.parse_for_expr(None, start),

            // Labeled loop `'label: loop/while/for/{`
            TokenKind::Lifetime(lbl) => {
                self.bump();
                self.expect(&TokenKind::Colon, "`:` after loop label")?;
                match self.peek_kind() {
                    TokenKind::Loop => self.parse_loop_expr(Some(lbl), start),
                    TokenKind::While => self.parse_while_expr(Some(lbl), start),
                    TokenKind::For => self.parse_for_expr(Some(lbl), start),
                    TokenKind::LBrace => {
                        let block = self.parse_block()?;
                        let span = start.merge(block.span);
                        Ok(Expr {
                            kind: ExprKind::Block(block),
                            span,
                        })
                    }
                    _ => Err(self.error("expected `loop`, `while`, `for`, or `{` after label")),
                }
            }

            TokenKind::Break => {
                self.bump();
                let label = if let TokenKind::Lifetime(lbl) = self.peek_kind().clone() {
                    self.bump();
                    Some(lbl)
                } else {
                    None
                };
                let value = if self.can_start_expr() && (allow_struct || !self.at(&TokenKind::LBrace)) {
                    Some(Box::new(self.parse_expr_bp(0, allow_struct)?))
                } else {
                    None
                };
                let end = value.as_ref().map(|e| e.span).unwrap_or(start);
                Ok(Expr {
                    kind: ExprKind::Break { label, value },
                    span: start.merge(end),
                })
            }
            TokenKind::Continue => {
                self.bump();
                let label = if let TokenKind::Lifetime(lbl) = self.peek_kind().clone() {
                    self.bump();
                    Some(lbl)
                } else {
                    None
                };
                let end = self.tokens[self.pos.saturating_sub(1)].span;
                Ok(Expr {
                    kind: ExprKind::Continue { label },
                    span: start.merge(end),
                })
            }
            TokenKind::Return => {
                self.bump();
                let value = if self.can_start_expr() {
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                let end = value.as_ref().map(|e| e.span).unwrap_or(start);
                Ok(Expr {
                    kind: ExprKind::Return(value),
                    span: start.merge(end),
                })
            }

            // Qualified path `<T as Trait>::method`
            TokenKind::Lt => {
                self.bump();
                let ty = self.parse_type()?;
                let as_trait = if self.eat(&TokenKind::As) {
                    Some(self.parse_path(true)?)
                } else {
                    None
                };
                self.expect_gt()?;
                self.expect(&TokenKind::ColonColon, "`::`")?;
                let (item, ispan) = self.parse_ident()?;
                // Optional trailing turbofish `::<T>`
                if self.at(&TokenKind::ColonColon) && matches!(self.peek_at(1), TokenKind::Lt) {
                    self.bump();
                    let _ = self.parse_generic_args()?;
                }
                Ok(Expr {
                    kind: ExprKind::QualifiedPath { ty, as_trait, item },
                    span: start.merge(ispan),
                })
            }

            // Paths, struct literals, and macro calls
            TokenKind::Ident(_)
            | TokenKind::ColonColon
            | TokenKind::SelfLower
            | TokenKind::SelfUpper
            | TokenKind::Super
            | TokenKind::Crate => {
                let path = self.parse_path(false)?;
                // Macro invocation `foo!(...)`
                if self.eat(&TokenKind::Bang) {
                    let (delim, tokens) = self.parse_delimited_tokens()?;
                    let end = self.tokens[self.pos.saturating_sub(1)].span;
                    return Ok(Expr {
                        kind: ExprKind::MacroCall(MacroCall {
                            path,
                            delim,
                            tokens,
                            span: start.merge(end),
                        }),
                        span: start.merge(end),
                    });
                }
                // Struct literal `Point { x: 1, y, ..other }`
                if allow_struct && self.at(&TokenKind::LBrace) && self.looks_like_struct_lit() {
                    return self.parse_struct_lit(path);
                }
                let span = path.span;
                Ok(Expr {
                    kind: ExprKind::Path(path),
                    span,
                })
            }

            _ => Err(self.error(format!("expected expression, found {:?}", self.peek_kind()))),
        }
    }

    fn looks_like_struct_lit(&self) -> bool {
        // After `{`:
        // `{}` -> empty struct literal
        // `{ ..expr }` -> struct literal
        // `{ ident : ... }` or `{ ident , ... }` or `{ ident }` -> struct literal
        if !self.at(&TokenKind::LBrace) {
            return false;
        }
        match self.peek_at(1) {
            TokenKind::RBrace | TokenKind::DotDot => true,
            TokenKind::Ident(_) => matches!(
                self.peek_at(2),
                TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace
            ),
            _ => false,
        }
    }

    fn parse_struct_lit(&mut self, path: Path) -> Result<Expr, ParseError> {
        let start = path.span;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        let mut rest = None;
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            if self.eat(&TokenKind::DotDot) {
                rest = Some(Box::new(self.parse_expr()?));
                let _ = self.eat(&TokenKind::Comma);
                break;
            }
            let (name, nspan) = self.parse_ident()?;
            let value = if self.eat(&TokenKind::Colon) {
                self.parse_expr()?
            } else {
                Expr {
                    kind: ExprKind::Path(Path::simple(name.clone(), nspan)),
                    span: nspan,
                }
            };
            let span = nspan.merge(value.span);
            fields.push(FieldInit { name, value, span });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Expr {
            kind: ExprKind::Struct { path, fields, rest },
            span: start.merge(end),
        })
    }

    fn parse_closure(
        &mut self,
        is_async: bool,
        is_move: bool,
        start: Span,
    ) -> Result<Expr, ParseError> {
        let mut params = Vec::new();
        if self.eat(&TokenKind::PipePipe) {
            // 0-parameter closure `|| ...`
        } else {
            self.expect(&TokenKind::Pipe, "`|`")?;
            while !self.at(&TokenKind::Pipe) && !self.at_eof() {
                let pat = self.parse_pat_no_or()?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push((pat, ty));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::Pipe, "`|`")?;
        }
        let (ret_type, body) = if self.eat(&TokenKind::Arrow) {
            let rt = self.parse_type()?;
            let blk = self.parse_block()?;
            let bspan = blk.span;
            (
                Some(rt),
                Expr {
                    kind: ExprKind::Block(blk),
                    span: bspan,
                },
            )
        } else {
            (None, self.parse_expr()?)
        };
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::Closure {
                is_async,
                is_move,
                params,
                ret_type,
                body: Box::new(body),
            },
            span,
        })
    }

    fn parse_if_cond(&mut self) -> Result<IfCond, ParseError> {
        let mut items = Vec::new();
        loop {
            if self.eat(&TokenKind::Let) {
                let pat = self.parse_pat()?;
                self.expect(&TokenKind::Eq, "`=`")?;
                // Stop before `&&` so let-chains parse cleanly
                let expr = self.parse_expr_bp(9, false)?;
                items.push(IfCond::Let { pat, expr });
            } else {
                // Stop before `&&` if followed by `&& let`
                let expr = if self.has_upcoming_and_let() {
                    self.parse_expr_bp(9, false)?
                } else {
                    self.parse_expr_no_struct()?
                };
                items.push(IfCond::Expr(expr));
            }
            if !self.eat(&TokenKind::AmpAmp) {
                break;
            }
        }
        if items.len() == 1 {
            Ok(items.into_iter().next().unwrap())
        } else {
            Ok(IfCond::Chain(items))
        }
    }

    fn has_upcoming_and_let(&self) -> bool {
        let mut i = self.pos;
        let mut depth = 0usize;
        while let Some(tok) = self.tokens.get(i) {
            match tok.kind {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
                TokenKind::LBrace | TokenKind::Semi | TokenKind::Eof if depth == 0 => return false,
                TokenKind::AmpAmp if depth == 0 => {
                    if matches!(self.tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::Let)) {
                        return true;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::If, "`if`")?.span;
        let cond = self.parse_if_cond()?;
        let then_block = self.parse_block()?;
        let mut end = then_block.span;
        let else_branch = if self.eat(&TokenKind::Else) {
            let e = if self.at(&TokenKind::If) {
                self.parse_if_expr()?
            } else {
                let blk = self.parse_block()?;
                let bspan = blk.span;
                Expr {
                    kind: ExprKind::Block(blk),
                    span: bspan,
                }
            };
            end = e.span;
            Some(Box::new(e))
        } else {
            None
        };
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_block,
                else_branch,
            },
            span: start.merge(end),
        })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::Match, "`match`")?.span;
        let scrutinee = self.parse_expr_no_struct()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        while self.is_inner_attr() {
            let _ = self.parse_attr(true)?;
        }
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let astart = self.peek().span;
            let attrs = self.parse_outer_attrs()?;
            let pat = self.parse_pat()?;
            let guard = if self.eat(&TokenKind::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow, "`=>`")?;
            let body = self.parse_stmt_expr()?;
            let aend = body.span;
            let needs_comma = !super::stmt::expr_needs_no_semi(&body.kind);
            if !self.eat(&TokenKind::Comma) && needs_comma && !self.at(&TokenKind::RBrace) {
                return Err(self.error("expected `,` or `}` after match arm"));
            }
            arms.push(MatchArm {
                attrs,
                pat,
                guard,
                body,
                span: astart.merge(aend),
            });
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: start.merge(end),
        })
    }

    fn parse_loop_expr(&mut self, label: Option<String>, start: Span) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::Loop, "`loop`")?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::Loop { label, body },
            span,
        })
    }

    fn parse_while_expr(&mut self, label: Option<String>, start: Span) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::While, "`while`")?;
        let cond = self.parse_if_cond()?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::While {
                label,
                cond: Box::new(cond),
                body,
            },
            span,
        })
    }

    fn parse_for_expr(&mut self, label: Option<String>, start: Span) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::For, "`for`")?;
        let pat = self.parse_pat()?;
        self.expect(&TokenKind::In, "`in`")?;
        let iter = self.parse_expr_no_struct()?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::For {
                label,
                pat,
                iter: Box::new(iter),
                body,
            },
            span,
        })
    }

    fn can_start_expr(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Int(_, _)
                | TokenKind::Float(_, _)
                | TokenKind::Char(_)
                | TokenKind::Byte(_)
                | TokenKind::Str(_)
                | TokenKind::ByteStr(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Ident(_)
                | TokenKind::SelfLower
                | TokenKind::SelfUpper
                | TokenKind::Super
                | TokenKind::Crate
                | TokenKind::ColonColon
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Minus
                | TokenKind::Bang
                | TokenKind::Star
                | TokenKind::Amp
                | TokenKind::AmpAmp
                | TokenKind::Pipe
                | TokenKind::PipePipe
                | TokenKind::Move
                | TokenKind::Async
                | TokenKind::Unsafe
                | TokenKind::If
                | TokenKind::Match
                | TokenKind::Loop
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Return
                | TokenKind::Lifetime(_)
                | TokenKind::DotDot
                | TokenKind::DotDotEq
                | TokenKind::Lt
        )
    }

    fn peek_assign_op(&self) -> Option<Option<BinOp>> {
        Some(match self.peek_kind() {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Rem),
            TokenKind::AmpEq => Some(BinOp::BitAnd),
            TokenKind::PipeEq => Some(BinOp::BitOr),
            TokenKind::CaretEq => Some(BinOp::BitXor),
            TokenKind::ShlEq => Some(BinOp::Shl),
            TokenKind::ShrEq => Some(BinOp::Shr),
            _ => return None,
        })
    }

    fn peek_bin_op(&self) -> Option<(BinOp, u8, u8)> {
        // Precedence table matching Rust Reference (low -> high):
        //  6: ||
        //  8: &&
        // 10: == != < <= > >=
        // 12: |
        // 14: ^
        // 16: &
        // 18: << >>
        // 20: + -
        // 22: * / %  (and `as` at 22)
        let (op, prec) = match self.peek_kind() {
            TokenKind::PipePipe => (BinOp::Or, 6),
            TokenKind::AmpAmp => (BinOp::And, 8),
            TokenKind::EqEq => (BinOp::Eq, 10),
            TokenKind::NotEq => (BinOp::Ne, 10),
            TokenKind::Lt => (BinOp::Lt, 10),
            TokenKind::Le => (BinOp::Le, 10),
            TokenKind::Gt => (BinOp::Gt, 10),
            TokenKind::Ge => (BinOp::Ge, 10),
            TokenKind::Pipe => (BinOp::BitOr, 12),
            TokenKind::Caret => (BinOp::BitXor, 14),
            TokenKind::Amp => (BinOp::BitAnd, 16),
            TokenKind::Shl => (BinOp::Shl, 18),
            TokenKind::Shr => (BinOp::Shr, 18),
            TokenKind::Plus => (BinOp::Add, 20),
            TokenKind::Minus => (BinOp::Sub, 20),
            TokenKind::Star => (BinOp::Mul, 22),
            TokenKind::Slash => (BinOp::Div, 22),
            TokenKind::Percent => (BinOp::Rem, 22),
            _ => return None,
        };
        Some((op, prec, prec + 1))
    }
}
