//! Expression parsing (Pratt precedence climbing + PHP 8.x constructs).

use crate::ast::*;
use crate::lexer::lex_php_expr;
use crate::parser::Parser;
use crate::token::{InterpChunk, Span, TokenKind};
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_bp(0)
    }

    pub(crate) fn parse_expr_with_attrs(&mut self, attrs: Vec<Attribute>) -> Result<Expr, ParseError> {
        let lhs = self.parse_prefix_with_attrs(attrs)?;
        self.parse_infix_loop(lhs, 0)
    }

    fn parse_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        self.enter()?;
        let res = (|| {
            let lhs = self.parse_prefix()?;
            self.parse_infix_loop(lhs, min_bp)
        })();
        self.leave();
        res
    }

    fn parse_infix_loop(&mut self, mut lhs: Expr, min_bp: u8) -> Result<Expr, ParseError> {
        loop {
            // Postfix operators (highest precedence: 40)
            lhs = self.parse_postfix(lhs)?;

            // Assignment operators (bp = 10, right-associative).
            // In PHP's grammar, `lvalue = expr` is valid on the RHS of higher-precedence
            // operators (`if ($a || $b = foo())`, `!$x = bar()`, `1 + $z = 5`, `null ?? $w = 9`),
            // while `and`/`or`/`xor` (bp 2..6) remain looser than `=` (`$a = true and false`).
            if let Some(aop) = self.peek_assign_op() {
                if !is_assign_target(&lhs) {
                    if 10 < min_bp {
                        break;
                    }
                    return Err(self.error_here("invalid left-hand side of assignment"));
                }
                self.advance();
                let op = if aop == AssignOp::Assign && self.eat(&TokenKind::Amp) {
                    AssignOp::AssignRef
                } else {
                    aop
                };
                let rhs = self.parse_bp(10)?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::Assign {
                        op,
                        target: Box::new(lhs),
                        value: Box::new(rhs),
                    },
                    span,
                };
                continue;
            }

            // Ternary `? :` and Elvis `?:` (bp = 14, left-associative in PHP)
            if matches!(self.kind(), TokenKind::Question) {
                if 14 < min_bp {
                    break;
                }
                self.advance();
                let then_expr = if self.eat(&TokenKind::Colon) {
                    None
                } else {
                    let e = self.parse_expr()?;
                    self.expect(&TokenKind::Colon, "':' in ternary expression")?;
                    Some(Box::new(e))
                };
                let else_expr = self.parse_bp(15)?;
                let span = lhs.span.merge(else_expr.span);
                lhs = Expr {
                    kind: ExprKind::Ternary {
                        cond: Box::new(lhs),
                        then_expr,
                        else_expr: Box::new(else_expr),
                    },
                    span,
                };
                continue;
            }

            // `instanceof` (bp = 35)
            if matches!(self.kind(), TokenKind::KwInstanceOf) {
                if 35 < min_bp {
                    break;
                }
                self.advance();
                let class = self.parse_class_ref()?;
                let span = lhs.span.merge(self.prev_span());
                lhs = Expr {
                    kind: ExprKind::InstanceOf {
                        expr: Box::new(lhs),
                        class,
                    },
                    span,
                };
                continue;
            }

            // Binary operators
            let Some((op, l_bp, r_bp)) = self.peek_bin_op() else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_bp(r_bp)?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn peek_assign_op(&self) -> Option<AssignOp> {
        Some(match self.kind() {
            TokenKind::Eq => AssignOp::Assign,
            TokenKind::PlusEq => AssignOp::Add,
            TokenKind::MinusEq => AssignOp::Sub,
            TokenKind::StarEq => AssignOp::Mul,
            TokenKind::SlashEq => AssignOp::Div,
            TokenKind::PercentEq => AssignOp::Mod,
            TokenKind::StarStarEq => AssignOp::Pow,
            TokenKind::DotEq => AssignOp::Concat,
            TokenKind::AmpEq => AssignOp::BitAnd,
            TokenKind::PipeEq => AssignOp::BitOr,
            TokenKind::CaretEq => AssignOp::BitXor,
            TokenKind::ShlEq => AssignOp::Shl,
            TokenKind::ShrEq => AssignOp::Shr,
            TokenKind::QuestionAssign => AssignOp::Coalesce,
            _ => return None,
        })
    }

    /// PHP 8.x binary operator binding powers `(op, left_bp, right_bp)`.
    fn peek_bin_op(&self) -> Option<(BinOp, u8, u8)> {
        Some(match self.kind() {
            TokenKind::KwOr => (BinOp::LogOr, 2, 3),
            TokenKind::KwXor => (BinOp::LogXor, 4, 5),
            TokenKind::KwAnd => (BinOp::LogAnd, 6, 7),
            // Assign is 10, Ternary is 14
            TokenKind::DoubleQuestion => (BinOp::Coalesce, 16, 16), // right-assoc
            TokenKind::PipePipe => (BinOp::LogOr, 18, 19),
            TokenKind::AmpAmp => (BinOp::LogAnd, 20, 21),
            TokenKind::Pipe => (BinOp::BitOr, 22, 23),
            TokenKind::Caret => (BinOp::BitXor, 24, 25),
            TokenKind::Amp => (BinOp::BitAnd, 26, 27),
            TokenKind::EqEq => (BinOp::Eq, 28, 29),
            TokenKind::BangEq => (BinOp::NotEq, 28, 29),
            TokenKind::EqEqEq => (BinOp::StrictEq, 28, 29),
            TokenKind::BangEqEq => (BinOp::StrictNotEq, 28, 29),
            TokenKind::Spaceship => (BinOp::Spaceship, 28, 29),
            TokenKind::Lt => (BinOp::Lt, 29, 30),
            TokenKind::LtEq => (BinOp::LtEq, 29, 30),
            TokenKind::Gt => (BinOp::Gt, 29, 30),
            TokenKind::GtEq => (BinOp::GtEq, 29, 30),
            TokenKind::Shl => (BinOp::Shl, 30, 31),
            TokenKind::Shr => (BinOp::Shr, 30, 31),
            // PHP 8.0+: `.` has lower precedence than `+`/`-` and higher than `<<`/`>>`!
            TokenKind::Dot => (BinOp::Concat, 31, 32),
            TokenKind::Plus => (BinOp::Add, 32, 33),
            TokenKind::Minus => (BinOp::Sub, 32, 33),
            TokenKind::Star => (BinOp::Mul, 34, 35),
            TokenKind::Slash => (BinOp::Div, 34, 35),
            TokenKind::Percent => (BinOp::Mod, 34, 35),
            // `**` is right-associative and binds tighter than unary `-`!
            TokenKind::StarStar => (BinOp::Pow, 38, 37),
            _ => return None,
        })
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.kind(), TokenKind::AttrOpen) {
            let attrs = self.parse_attributes()?;
            return self.parse_prefix_with_attrs(attrs);
        }
        self.parse_prefix_with_attrs(Vec::new())
    }

    fn parse_prefix_with_attrs(&mut self, attrs: Vec<Attribute>) -> Result<Expr, ParseError> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::KwNull => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Null, span: sp })
            }
            TokenKind::KwTrue => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Bool(true), span: sp })
            }
            TokenKind::KwFalse => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Bool(false), span: sp })
            }
            TokenKind::IntLit(v) => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Int(v), span: sp })
            }
            TokenKind::FloatLit(v) => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Float(v), span: sp })
            }
            TokenKind::StringLit(s) => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::String(s), span: sp })
            }
            TokenKind::InterpString(chunks) => {
                let sp = self.advance().span;
                let mut parts = Vec::with_capacity(chunks.len());
                for chunk in chunks {
                    match chunk {
                        InterpChunk::Literal(s) => parts.push(StringPart::Literal(s)),
                        InterpChunk::ExprSource { source, offset } => {
                            let sub_toks = lex_php_expr(&source, offset, self.limits)?;
                            let mut sub_p = Parser {
                                source: &source,
                                tokens: &sub_toks,
                                pos: 0,
                                depth: self.depth + 1,
                                limits: self.limits,
                            };
                            let expr = sub_p.parse_expr()?;
                            parts.push(StringPart::Expr(expr));
                        }
                    }
                }
                Ok(Expr {
                    kind: ExprKind::InterpString(parts),
                    span: sp,
                })
            }
            TokenKind::MagicFile => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::File), span: self.advance().span }),
            TokenKind::MagicDir => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Dir), span: self.advance().span }),
            TokenKind::MagicLine => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Line), span: self.advance().span }),
            TokenKind::MagicFunction => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Function), span: self.advance().span }),
            TokenKind::MagicClass => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Class), span: self.advance().span }),
            TokenKind::MagicMethod => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Method), span: self.advance().span }),
            TokenKind::MagicNamespace => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Namespace), span: self.advance().span }),
            TokenKind::MagicTrait => Ok(Expr { kind: ExprKind::MagicConst(MagicConst::Trait), span: self.advance().span }),

            TokenKind::Var(name) => {
                let sp = self.advance().span;
                Ok(Expr { kind: ExprKind::Var(name), span: sp })
            }
            TokenKind::Dollar => {
                self.advance();
                let inner = if self.eat(&TokenKind::LBrace) {
                    let e = self.parse_expr()?;
                    self.expect(&TokenKind::RBrace, "'}' closing dynamic variable")?;
                    e
                } else {
                    self.parse_prefix()?
                };
                let span = start.merge(inner.span);
                Ok(Expr {
                    kind: ExprKind::DynamicVar(Box::new(inner)),
                    span,
                })
            }

            // Unary operators (bp = 36 so `**` at 38 binds tighter: `-2 ** 2 == -(2 ** 2)`!)
            TokenKind::Plus => {
                self.advance();
                let e = self.parse_bp(36)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::Plus, expr: Box::new(e) } })
            }
            TokenKind::Minus => {
                self.advance();
                let e = self.parse_bp(36)?;
                // Special case: `-9223372036854775808` is `i64::MIN` in PHP!
                if let ExprKind::Float(f) = e.kind {
                    if f == 9223372036854775808.0 {
                        return Ok(Expr {
                            kind: ExprKind::Int(i64::MIN),
                            span: start.merge(e.span),
                        });
                    }
                }
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::Neg, expr: Box::new(e) } })
            }
            TokenKind::Bang => {
                self.advance();
                let e = self.parse_bp(34)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(e) } })
            }
            TokenKind::Tilde => {
                self.advance();
                let e = self.parse_bp(36)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::BitNot, expr: Box::new(e) } })
            }
            TokenKind::At => {
                self.advance();
                let e = self.parse_bp(36)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::Suppress, expr: Box::new(e) } })
            }
            TokenKind::PlusPlus => {
                self.advance();
                let e = self.parse_bp(36)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::PreInc, expr: Box::new(e) } })
            }
            TokenKind::MinusMinus => {
                self.advance();
                let e = self.parse_bp(36)?;
                Ok(Expr { span: start.merge(e.span), kind: ExprKind::Unary { op: UnaryOp::PreDec, expr: Box::new(e) } })
            }

            // Parenthesized expression or PHP cast `(int)$x`
            TokenKind::LParen => {
                if let Some(cast_kind) = self.try_peek_cast() {
                    self.advance(); // (
                    self.advance(); // cast word
                    self.advance(); // )
                    let e = self.parse_bp(36)?;
                    return Ok(Expr {
                        span: start.merge(e.span),
                        kind: ExprKind::Cast {
                            kind: cast_kind,
                            expr: Box::new(e),
                        },
                    });
                }
                self.advance();
                let mut e = self.parse_expr()?;
                let end = self.expect(&TokenKind::RParen, "')'")?;
                e.span = start.merge(end);
                Ok(e)
            }

            // Short array literal `[...]`
            TokenKind::LBracket => self.parse_array_lit(start, TokenKind::RBracket),
            // Classic `array(...)`
            TokenKind::KwArray => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after array")?;
                self.parse_array_items(start, TokenKind::RParen, false)
            }
            // `list(...)`
            TokenKind::KwList => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after list")?;
                self.parse_array_items(start, TokenKind::RParen, true)
            }

            // `new Class(...)` or anonymous `new class(...) ... {}`
            TokenKind::KwNew => self.parse_new(start),

            // `clone $obj`
            TokenKind::KwClone => {
                self.advance();
                let e = self.parse_bp(39)?;
                Ok(Expr {
                    span: start.merge(e.span),
                    kind: ExprKind::Clone(Box::new(e)),
                })
            }

            // Closure `function (...) use (...) { ... }` or static closure/arrow fn
            TokenKind::KwStatic
                if matches!(self.peek_at(1).kind, TokenKind::KwFunction | TokenKind::KwFn) =>
            {
                self.advance();
                if matches!(self.kind(), TokenKind::KwFn) {
                    self.parse_arrow_fn(attrs, true, start)
                } else {
                    self.parse_closure(attrs, true, start)
                }
            }
            TokenKind::KwFunction => self.parse_closure(attrs, false, start),
            TokenKind::KwFn => self.parse_arrow_fn(attrs, false, start),

            // PHP 8 `match ($x) { ... }`
            TokenKind::KwMatch if matches!(self.peek_at(1).kind, TokenKind::LParen) => {
                self.parse_match(start)
            }

            // PHP 8 `throw $e` expression
            TokenKind::KwThrow => {
                self.advance();
                let e = self.parse_bp(10)?;
                Ok(Expr {
                    span: start.merge(e.span),
                    kind: ExprKind::Throw(Box::new(e)),
                })
            }

            // `yield` / `yield $k => $v` / `yield from $gen`
            TokenKind::KwYield => {
                self.advance();
                if self.eat(&TokenKind::KwFrom) {
                    let e = self.parse_bp(11)?;
                    return Ok(Expr {
                        span: start.merge(e.span),
                        kind: ExprKind::YieldFrom(Box::new(e)),
                    });
                }
                if matches!(
                    self.kind(),
                    TokenKind::Semi | TokenKind::RParen | TokenKind::RBracket | TokenKind::Comma | TokenKind::CloseTag | TokenKind::Eof
                ) {
                    return Ok(Expr {
                        kind: ExprKind::Yield { key: None, value: None },
                        span: start,
                    });
                }
                let first = self.parse_bp(11)?;
                if self.eat(&TokenKind::FatArrow) {
                    let val = self.parse_bp(11)?;
                    let span = start.merge(val.span);
                    Ok(Expr {
                        kind: ExprKind::Yield {
                            key: Some(Box::new(first)),
                            value: Some(Box::new(val)),
                        },
                        span,
                    })
                } else {
                    let span = start.merge(first.span);
                    Ok(Expr {
                        kind: ExprKind::Yield {
                            key: None,
                            value: Some(Box::new(first)),
                        },
                        span,
                    })
                }
            }

            // `isset($a, $b)`
            TokenKind::KwIsset => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after isset")?;
                let mut args = vec![self.parse_expr()?];
                while self.eat(&TokenKind::Comma) {
                    if matches!(self.kind(), TokenKind::RParen) {
                        break;
                    }
                    args.push(self.parse_expr()?);
                }
                let end = self.expect(&TokenKind::RParen, "')' after isset")?;
                Ok(Expr {
                    kind: ExprKind::Isset(args),
                    span: start.merge(end),
                })
            }

            // `empty($a)`
            TokenKind::KwEmpty => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after empty")?;
                let e = self.parse_expr()?;
                let end = self.expect(&TokenKind::RParen, "')' after empty")?;
                Ok(Expr {
                    kind: ExprKind::Empty(Box::new(e)),
                    span: start.merge(end),
                })
            }

            // `print expr`
            TokenKind::KwPrint => {
                self.advance();
                let e = self.parse_bp(8)?;
                Ok(Expr {
                    span: start.merge(e.span),
                    kind: ExprKind::Print(Box::new(e)),
                })
            }

            // `include` / `include_once` / `require` / `require_once`
            TokenKind::KwInclude
            | TokenKind::KwIncludeOnce
            | TokenKind::KwRequire
            | TokenKind::KwRequireOnce => {
                let kind = match self.advance().kind {
                    TokenKind::KwInclude => IncludeKind::Include,
                    TokenKind::KwIncludeOnce => IncludeKind::IncludeOnce,
                    TokenKind::KwRequire => IncludeKind::Require,
                    _ => IncludeKind::RequireOnce,
                };
                let e = self.parse_bp(8)?;
                Ok(Expr {
                    span: start.merge(e.span),
                    kind: ExprKind::Include {
                        kind,
                        expr: Box::new(e),
                    },
                })
            }

            // `exit` / `die`
            TokenKind::KwExit | TokenKind::KwDie => {
                self.advance();
                let arg = if self.eat(&TokenKind::LParen) {
                    let v = if matches!(self.kind(), TokenKind::RParen) {
                        None
                    } else {
                        Some(Box::new(self.parse_expr()?))
                    };
                    self.expect(&TokenKind::RParen, "')' after exit/die")?;
                    v
                } else {
                    None
                };
                Ok(Expr {
                    kind: ExprKind::Exit(arg),
                    span: start.merge(self.prev_span()),
                })
            }

            // `eval($code)`
            TokenKind::KwEval => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after eval")?;
                let e = self.parse_expr()?;
                let end = self.expect(&TokenKind::RParen, "')' after eval")?;
                Ok(Expr {
                    kind: ExprKind::Eval(Box::new(e)),
                    span: start.merge(end),
                })
            }

            // Qualified / unqualified name: `Foo`, `\Foo\Bar`, `namespace\Foo`, `static::...`, `self::...`, `parent::...`
            TokenKind::Ident(_)
            | TokenKind::Backslash
            | TokenKind::KwNamespace
            | TokenKind::KwStatic
            | TokenKind::KwFrom
            | TokenKind::KwMatch
            | TokenKind::KwReadonly
            | TokenKind::KwEnum => {
                let name = self.parse_name()?;
                let span = name.span;
                Ok(Expr {
                    kind: ExprKind::ConstFetch(name),
                    span,
                })
            }

            other => Err(self.error_here(&format!("unexpected token in expression: {other:?}"))),
        }
    }

    fn try_peek_cast(&self) -> Option<CastKind> {
        if !matches!(self.kind(), TokenKind::LParen) {
            return None;
        }
        if !matches!(self.peek_at(2).kind, TokenKind::RParen) {
            return None;
        }
        match &self.peek_at(1).kind {
            TokenKind::KwArray => Some(CastKind::Array),
            TokenKind::KwUnset => Some(CastKind::Unset),
            TokenKind::Ident(s) => match s.to_ascii_lowercase().as_str() {
                "int" | "integer" => Some(CastKind::Int),
                "float" | "double" | "real" => Some(CastKind::Float),
                "string" | "binary" => Some(CastKind::String),
                "bool" | "boolean" => Some(CastKind::Bool),
                "object" => Some(CastKind::Object),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_postfix(&mut self, mut expr: Expr) -> Result<Expr, ParseError> {
        loop {
            match self.kind() {
                // Function call `f(...)`
                TokenKind::LParen => {
                    let args = self.parse_call_args()?;
                    let span = expr.span.merge(self.prev_span());
                    let callee = match expr.kind {
                        ExprKind::ConstFetch(name) => Callee::Name(name),
                        _ => Callee::Expr(Box::new(expr)),
                    };
                    expr = Expr {
                        kind: ExprKind::Call { callee, args },
                        span,
                    };
                }
                // Array index `$a[i]` or append `$a[]`
                TokenKind::LBracket => {
                    self.advance();
                    let index = if matches!(self.kind(), TokenKind::RBracket) {
                        None
                    } else {
                        Some(Box::new(self.parse_expr()?))
                    };
                    let end = self.expect(&TokenKind::RBracket, "']'")?;
                    let span = expr.span.merge(end);
                    expr = Expr {
                        kind: ExprKind::Index {
                            target: Box::new(expr),
                            index,
                        },
                        span,
                    };
                }
                // Property or method call `$o->p`, `$o?->p`, `$o->m(...)`, `$o?->m(...)`
                TokenKind::Arrow | TokenKind::NullsafeArrow => {
                    let nullsafe = matches!(self.advance().kind, TokenKind::NullsafeArrow);
                    let prop = self.parse_prop_name()?;
                    if matches!(self.kind(), TokenKind::LParen) {
                        let args = self.parse_call_args()?;
                        let span = expr.span.merge(self.prev_span());
                        expr = Expr {
                            kind: ExprKind::MethodCall {
                                target: Box::new(expr),
                                method: prop,
                                nullsafe,
                                args,
                            },
                            span,
                        };
                    } else {
                        let span = expr.span.merge(self.prev_span());
                        expr = Expr {
                            kind: ExprKind::Prop {
                                target: Box::new(expr),
                                prop,
                                nullsafe,
                            },
                            span,
                        };
                    }
                }
                // Static member access `Cls::CONST`, `Cls::$prop`, `Cls::method(...)`, `Cls::class`
                TokenKind::DoubleColon => {
                    self.advance();
                    let class = match expr.kind {
                        ExprKind::ConstFetch(name) => ClassRef::Name(name),
                        _ => ClassRef::Expr(Box::new(expr)),
                    };
                    if let TokenKind::Var(v) = self.kind().clone() {
                        let end = self.advance().span;
                        if matches!(self.kind(), TokenKind::LParen) {
                            // `Cls::$method(...)`
                            let method = PropName::Expr(Box::new(Expr {
                                kind: ExprKind::Var(v),
                                span: end,
                            }));
                            let args = self.parse_call_args()?;
                            expr = Expr {
                                span: class_span(&class).merge(self.prev_span()),
                                kind: ExprKind::StaticCall { class, method, args },
                            };
                        } else {
                            expr = Expr {
                                span: class_span(&class).merge(end),
                                kind: ExprKind::StaticProp {
                                    class,
                                    prop: PropName::Ident(v),
                                },
                            };
                        }
                    } else if matches!(self.kind(), TokenKind::Dollar) {
                        // `Cls::${expr}`
                        self.advance();
                        self.expect(&TokenKind::LBrace, "'{' after ::$")?;
                        let pexpr = self.parse_expr()?;
                        let end = self.expect(&TokenKind::RBrace, "'}'")?;
                        expr = Expr {
                            span: class_span(&class).merge(end),
                            kind: ExprKind::StaticProp {
                                class,
                                prop: PropName::Expr(Box::new(pexpr)),
                            },
                        };
                    } else if matches!(self.kind(), TokenKind::LBrace) {
                        // `Cls::{$expr}(...)` or PHP 8.3 dynamic class constant fetch `Cls::{$expr}`
                        self.advance();
                        let mexpr = self.parse_expr()?;
                        let rbrace = self.expect(&TokenKind::RBrace, "'}'")?;
                        if matches!(self.kind(), TokenKind::LParen) {
                            let args = self.parse_call_args()?;
                            expr = Expr {
                                span: class_span(&class).merge(self.prev_span()),
                                kind: ExprKind::StaticCall {
                                    class,
                                    method: PropName::Expr(Box::new(mexpr)),
                                    args,
                                },
                            };
                        } else {
                            expr = Expr {
                                span: class_span(&class).merge(rbrace),
                                kind: ExprKind::ClassConst {
                                    class,
                                    name: PropName::Expr(Box::new(mexpr)),
                                },
                            };
                        }
                    } else {
                        let (member, mend) = self.expect_member_name()?;
                        if matches!(self.kind(), TokenKind::LParen) {
                            let args = self.parse_call_args()?;
                            expr = Expr {
                                span: class_span(&class).merge(self.prev_span()),
                                kind: ExprKind::StaticCall {
                                    class,
                                    method: PropName::Ident(member),
                                    args,
                                },
                            };
                        } else {
                            expr = Expr {
                                span: class_span(&class).merge(mend),
                                kind: ExprKind::ClassConst {
                                    class,
                                    name: PropName::Ident(member),
                                },
                            };
                        }
                    }
                }
                TokenKind::PlusPlus => {
                    let end = self.advance().span;
                    let span = expr.span.merge(end);
                    expr = Expr {
                        kind: ExprKind::Unary {
                            op: UnaryOp::PostInc,
                            expr: Box::new(expr),
                        },
                        span,
                    };
                }
                TokenKind::MinusMinus => {
                    let end = self.advance().span;
                    let span = expr.span.merge(end);
                    expr = Expr {
                        kind: ExprKind::Unary {
                            op: UnaryOp::PostDec,
                            expr: Box::new(expr),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_prop_name(&mut self) -> Result<PropName, ParseError> {
        if self.eat(&TokenKind::LBrace) {
            let e = self.parse_expr()?;
            self.expect(&TokenKind::RBrace, "'}' closing dynamic property name")?;
            return Ok(PropName::Expr(Box::new(e)));
        }
        if let TokenKind::Var(v) = self.kind().clone() {
            let sp = self.advance().span;
            return Ok(PropName::Expr(Box::new(Expr {
                kind: ExprKind::Var(v),
                span: sp,
            })));
        }
        let (name, _) = self.expect_member_name()?;
        Ok(PropName::Ident(name))
    }

    fn parse_class_ref(&mut self) -> Result<ClassRef, ParseError> {
        let mut e = match self.kind() {
            TokenKind::Var(_) | TokenKind::Dollar | TokenKind::LParen => self.parse_prefix()?,
            _ => {
                let name = self.parse_name()?;
                // PHP `new_variable`: `ClassName::$prop`, `static::$prop`, `self::$prop`
                // e.g. `new static::$responseFactoryClass()` or `$x instanceof static::$cls`.
                if matches!(self.kind(), TokenKind::DoubleColon)
                    && matches!(self.peek_at(1).kind, TokenKind::Var(_) | TokenKind::Dollar)
                {
                    let cspan = name.span;
                    self.advance(); // ::
                    let (prop, end) = self.parse_static_prop_name()?;
                    Expr {
                        kind: ExprKind::StaticProp {
                            class: ClassRef::Name(name),
                            prop,
                        },
                        span: cspan.merge(end),
                    }
                } else {
                    return Ok(ClassRef::Name(name));
                }
            }
        };
        // Allow chained property / index / static-property on class variable (`new_variable`),
        // e.g. `new $this->cls()`, `new static::$map['k']()`, `new $a::$b()`.
        loop {
            match self.kind() {
                TokenKind::Arrow | TokenKind::NullsafeArrow => {
                    let nullsafe = matches!(self.kind(), TokenKind::NullsafeArrow);
                    self.advance();
                    let prop = self.parse_prop_name()?;
                    let span = e.span.merge(self.prev_span());
                    e = Expr {
                        kind: ExprKind::Prop {
                            target: Box::new(e),
                            prop,
                            nullsafe,
                        },
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    let idx = self.parse_expr()?;
                    let end = self.expect(&TokenKind::RBracket, "']'")?;
                    let span = e.span.merge(end);
                    e = Expr {
                        kind: ExprKind::Index {
                            target: Box::new(e),
                            index: Some(Box::new(idx)),
                        },
                        span,
                    };
                }
                TokenKind::DoubleColon
                    if matches!(self.peek_at(1).kind, TokenKind::Var(_) | TokenKind::Dollar) =>
                {
                    self.advance(); // ::
                    let (prop, end) = self.parse_static_prop_name()?;
                    let span = e.span.merge(end);
                    e = Expr {
                        kind: ExprKind::StaticProp {
                            class: ClassRef::Expr(Box::new(e)),
                            prop,
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(ClassRef::Expr(Box::new(e)))
    }

    fn parse_static_prop_name(&mut self) -> Result<(PropName, Span), ParseError> {
        if let TokenKind::Var(v) = self.kind().clone() {
            let end = self.advance().span;
            return Ok((PropName::Ident(v), end));
        }
        self.expect(&TokenKind::Dollar, "'$' in static property name")?;
        if self.eat(&TokenKind::LBrace) {
            let pexpr = self.parse_expr()?;
            let end = self.expect(&TokenKind::RBrace, "'}'")?;
            Ok((PropName::Expr(Box::new(pexpr)), end))
        } else {
            let pexpr = self.parse_prefix()?;
            let end = pexpr.span;
            Ok((PropName::Expr(Box::new(pexpr)), end))
        }
    }

    pub(crate) fn parse_call_args(&mut self) -> Result<CallArgs, ParseError> {
        self.expect(&TokenKind::LParen, "'(' starting argument list")?;
        // PHP 8.1 first-class callable `foo(...)`
        if matches!(self.kind(), TokenKind::Ellipsis)
            && matches!(self.peek_at(1).kind, TokenKind::RParen)
        {
            self.advance();
            self.advance();
            return Ok(CallArgs::FirstClassCallable);
        }
        let mut args = Vec::new();
        while !matches!(self.kind(), TokenKind::RParen | TokenKind::Eof) {
            let astart = self.span();
            let name = if self.kind().as_member_name().is_some()
                && matches!(self.peek_at(1).kind, TokenKind::Colon)
            {
                let (n, _) = self.expect_member_name()?;
                self.advance(); // :
                Some(n)
            } else {
                None
            };
            let unpack = self.eat(&TokenKind::Ellipsis);
            let value = self.parse_expr()?;
            args.push(Arg {
                name,
                unpack,
                span: astart.merge(value.span),
                value,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')' closing argument list")?;
        Ok(CallArgs::Normal(args))
    }

    fn parse_array_lit(&mut self, start: Span, close: TokenKind) -> Result<Expr, ParseError> {
        self.advance(); // [
        self.parse_array_items(start, close, false)
    }

    fn parse_array_items(
        &mut self,
        start: Span,
        close: TokenKind,
        is_list: bool,
    ) -> Result<Expr, ParseError> {
        let mut items = Vec::new();
        while self.kind() != &close && !self.is_eof() {
            let istart = self.span();
            // Empty slot in destructuring: `[, $b] = $arr` or `list(, $b) = $arr`
            if self.eat(&TokenKind::Comma) {
                items.push(ArrayItem {
                    key: None,
                    value: Expr {
                        kind: ExprKind::Null,
                        span: istart,
                    },
                    by_ref: false,
                    unpack: false,
                    span: istart,
                });
                continue;
            }
            if self.eat(&TokenKind::Ellipsis) {
                let value = self.parse_expr()?;
                items.push(ArrayItem {
                    key: None,
                    span: istart.merge(value.span),
                    value,
                    by_ref: false,
                    unpack: true,
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                continue;
            }
            let by_ref = self.eat(&TokenKind::Amp);
            let first = self.parse_expr()?;
            let (key, value, val_by_ref) = if self.eat(&TokenKind::FatArrow) {
                let r = self.eat(&TokenKind::Amp);
                let v = self.parse_expr()?;
                (Some(first), v, r)
            } else {
                (None, first, by_ref)
            };
            items.push(ArrayItem {
                key,
                span: istart.merge(value.span),
                value,
                by_ref: val_by_ref,
                unpack: false,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&close, "closing array delimiter")?;
        Ok(Expr {
            kind: if is_list {
                ExprKind::List(items)
            } else {
                ExprKind::Array(items)
            },
            span: start.merge(end),
        })
    }

    fn parse_new(&mut self, start: Span) -> Result<Expr, ParseError> {
        self.advance(); // new
        // Anonymous class: `new [readonly] class[(args)] [extends ...] [implements ...] { ... }`
        let is_anon = matches!(self.kind(), TokenKind::KwClass)
            || (matches!(self.kind(), TokenKind::KwReadonly)
                && matches!(self.peek_at(1).kind, TokenKind::KwClass));
        if is_anon {
            let is_readonly = self.eat(&TokenKind::KwReadonly);
            self.advance(); // class
            let args = if matches!(self.kind(), TokenKind::LParen) {
                match self.parse_call_args()? {
                    CallArgs::Normal(a) => a,
                    CallArgs::FirstClassCallable => {
                        return Err(self.error_here("first-class callable not allowed in new"));
                    }
                }
            } else {
                Vec::new()
            };
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
            return Ok(Expr {
                kind: ExprKind::AnonClass {
                    is_readonly,
                    extends,
                    implements,
                    args,
                    members,
                },
                span: start.merge(end),
            });
        }

        let class = self.parse_class_ref()?;
        let args = if matches!(self.kind(), TokenKind::LParen) {
            match self.parse_call_args()? {
                CallArgs::Normal(a) => a,
                CallArgs::FirstClassCallable => {
                    return Err(self.error_here("first-class callable not allowed in new"));
                }
            }
        } else {
            Vec::new()
        };
        Ok(Expr {
            kind: ExprKind::New { class, args },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_closure(
        &mut self,
        attributes: Vec<Attribute>,
        is_static: bool,
        start: Span,
    ) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::KwFunction, "'function'")?;
        let by_ref = self.eat(&TokenKind::Amp);
        let params = self.parse_params(false)?;
        let mut uses = Vec::new();
        if self.eat(&TokenKind::KwUse) {
            self.expect(&TokenKind::LParen, "'(' after use")?;
            while !matches!(self.kind(), TokenKind::RParen | TokenKind::Eof) {
                let ustart = self.span();
                let uref = self.eat(&TokenKind::Amp);
                let (name, end) = self.expect_var()?;
                uses.push(ClosureUse {
                    name,
                    by_ref: uref,
                    span: ustart.merge(end),
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')' closing use list")?;
        }
        let return_type = self.parse_return_type()?;
        self.expect(&TokenKind::LBrace, "'{' starting closure body")?;
        let mut body = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        let end = self.expect(&TokenKind::RBrace, "'}' closing closure body")?;
        Ok(Expr {
            kind: ExprKind::Closure {
                attributes,
                is_static,
                by_ref,
                params,
                uses,
                return_type,
                body,
            },
            span: start.merge(end),
        })
    }

    fn parse_arrow_fn(
        &mut self,
        attributes: Vec<Attribute>,
        is_static: bool,
        start: Span,
    ) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::KwFn, "'fn'")?;
        let by_ref = self.eat(&TokenKind::Amp);
        let params = self.parse_params(false)?;
        let return_type = self.parse_return_type()?;
        self.expect(&TokenKind::FatArrow, "'=>' in arrow function")?;
        let body = self.parse_expr()?;
        let span = start.merge(body.span);
        Ok(Expr {
            kind: ExprKind::ArrowFn {
                attributes,
                is_static,
                by_ref,
                params,
                return_type,
                body: Box::new(body),
            },
            span,
        })
    }

    fn parse_match(&mut self, start: Span) -> Result<Expr, ParseError> {
        self.advance(); // match
        self.expect(&TokenKind::LParen, "'(' after match")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')' after match subject")?;
        self.expect(&TokenKind::LBrace, "'{' starting match arms")?;
        let mut arms = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            let astart = self.span();
            let conds = if self.eat(&TokenKind::KwDefault) {
                self.eat(&TokenKind::Comma);
                None
            } else {
                let mut list = vec![self.parse_expr()?];
                while self.eat(&TokenKind::Comma) {
                    if matches!(self.kind(), TokenKind::FatArrow) {
                        break;
                    }
                    list.push(self.parse_expr()?);
                }
                Some(list)
            };
            self.expect(&TokenKind::FatArrow, "'=>' in match arm")?;
            let body = self.parse_expr()?;
            arms.push(MatchArm {
                conds,
                span: astart.merge(body.span),
                body,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "'}' closing match")?;
        Ok(Expr {
            kind: ExprKind::Match {
                cond: Box::new(cond),
                arms,
            },
            span: start.merge(end),
        })
    }
}

fn is_assign_target(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Var(_)
            | ExprKind::DynamicVar(_)
            | ExprKind::Index { .. }
            | ExprKind::Prop { .. }
            | ExprKind::StaticProp { .. }
            | ExprKind::Array(_)
            | ExprKind::List(_)
    )
}

fn class_span(c: &ClassRef) -> Span {
    match c {
        ClassRef::Name(n) => n.span,
        ClassRef::Expr(e) => e.span,
    }
}
