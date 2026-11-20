//! Statement parsing for PHP 8.x.

use crate::ast::*;
use crate::parser::Parser;
use crate::token::{Span, TokenKind};
use crate::ParseError;

impl<'a> Parser<'a> {
    pub(crate) fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.enter()?;
        let res = self.parse_stmt_inner();
        self.leave();
        res
    }

    fn parse_stmt_inner(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::InlineHtml(html) => {
                let sp = self.advance().span;
                Ok(Stmt {
                    kind: StmtKind::InlineHtml(html),
                    span: sp,
                })
            }
            TokenKind::OpenEchoTag => {
                self.advance();
                let mut exprs = vec![self.parse_expr()?];
                while self.eat(&TokenKind::Comma) {
                    exprs.push(self.parse_expr()?);
                }
                self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Echo(exprs),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::CloseTag | TokenKind::Semi => {
                let sp = self.advance().span;
                Ok(Stmt {
                    kind: StmtKind::Nop,
                    span: sp,
                })
            }
            TokenKind::LBrace => {
                self.advance();
                let mut stmts = Vec::new();
                while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                    stmts.push(self.parse_stmt()?);
                }
                let end = self.expect(&TokenKind::RBrace, "'}'")?;
                Ok(Stmt {
                    kind: StmtKind::Block(stmts),
                    span: start.merge(end),
                })
            }
            TokenKind::AttrOpen => {
                let attrs = self.parse_attributes()?;
                self.parse_attributed_stmt(attrs, start)
            }
            TokenKind::KwEcho => {
                self.advance();
                let mut exprs = vec![self.parse_expr()?];
                while self.eat(&TokenKind::Comma) {
                    exprs.push(self.parse_expr()?);
                }
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Echo(exprs),
                    span: start.merge(end),
                })
            }
            TokenKind::KwReturn => {
                self.advance();
                let val = if matches!(self.kind(), TokenKind::Semi | TokenKind::CloseTag | TokenKind::Eof) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Return(val),
                    span: start.merge(end),
                })
            }
            TokenKind::KwBreak => {
                self.advance();
                let level = if matches!(self.kind(), TokenKind::Semi | TokenKind::CloseTag | TokenKind::Eof) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Break(level),
                    span: start.merge(end),
                })
            }
            TokenKind::KwContinue => {
                self.advance();
                let level = if matches!(self.kind(), TokenKind::Semi | TokenKind::CloseTag | TokenKind::Eof) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Continue(level),
                    span: start.merge(end),
                })
            }
            TokenKind::KwGlobal => {
                self.advance();
                let mut vars = Vec::new();
                loop {
                    let (v, _) = self.expect_var()?;
                    vars.push(v);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Global(vars),
                    span: start.merge(end),
                })
            }
            TokenKind::KwStatic if matches!(self.peek_at(1).kind, TokenKind::Var(_)) => {
                self.advance();
                let mut items = Vec::new();
                loop {
                    let (v, _) = self.expect_var()?;
                    let init = if self.eat(&TokenKind::Eq) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    items.push((v, init));
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::StaticVar(items),
                    span: start.merge(end),
                })
            }
            TokenKind::KwUnset => {
                self.advance();
                self.expect(&TokenKind::LParen, "'(' after unset")?;
                let mut targets = Vec::new();
                if !matches!(self.kind(), TokenKind::RParen) {
                    targets.push(self.parse_expr()?);
                    while self.eat(&TokenKind::Comma) {
                        if matches!(self.kind(), TokenKind::RParen) {
                            break;
                        }
                        targets.push(self.parse_expr()?);
                    }
                }
                self.expect(&TokenKind::RParen, "')' after unset")?;
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Unset(targets),
                    span: start.merge(end),
                })
            }
            TokenKind::KwIf => self.parse_if(start),
            TokenKind::KwWhile => self.parse_while(start),
            TokenKind::KwDo => self.parse_do_while(start),
            TokenKind::KwFor => self.parse_for(start),
            TokenKind::KwForEach => self.parse_foreach(start),
            TokenKind::KwSwitch => self.parse_switch(start),
            TokenKind::KwTry => self.parse_try(start),
            TokenKind::KwNamespace if !matches!(self.peek_at(1).kind, TokenKind::Backslash) => {
                self.parse_namespace(start)
            }
            TokenKind::KwUse => self.parse_use(start),
            TokenKind::KwConst => {
                self.advance();
                let mut items = Vec::new();
                loop {
                    let (name, _) = self.expect_member_name()?;
                    self.expect(&TokenKind::Eq, "'=' in const declaration")?;
                    let val = self.parse_expr()?;
                    items.push((name, val));
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Const(items),
                    span: start.merge(end),
                })
            }
            TokenKind::KwDeclare => self.parse_declare(start),
            TokenKind::KwGoto => {
                self.advance();
                let (label, _) = self.expect_ident()?;
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Goto(label),
                    span: start.merge(end),
                })
            }
            // Function declaration vs closure expression: `function foo(...)` vs `function (...)`
            TokenKind::KwFunction if self.is_named_function_decl() => {
                let decl = self.parse_function_decl(Vec::new(), start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Function(decl),
                })
            }
            TokenKind::KwClass
            | TokenKind::KwAbstract
            | TokenKind::KwFinal
            | TokenKind::KwReadonly
                if self.is_class_decl_start() =>
            {
                let decl = self.parse_class_decl(Vec::new(), start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Class(decl),
                })
            }
            TokenKind::KwInterface => {
                let decl = self.parse_interface_decl(Vec::new(), start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Interface(decl),
                })
            }
            TokenKind::KwTrait => {
                let decl = self.parse_trait_decl(Vec::new(), start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Trait(decl),
                })
            }
            TokenKind::KwEnum if matches!(self.peek_at(1).kind, TokenKind::Ident(_)) => {
                let decl = self.parse_enum_decl(Vec::new(), start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Enum(decl),
                })
            }
            // Label `foo:`
            TokenKind::Ident(ref name) if matches!(self.peek_at(1).kind, TokenKind::Colon) => {
                let label = name.clone();
                self.advance();
                let end = self.advance().span;
                Ok(Stmt {
                    kind: StmtKind::Label(label),
                    span: start.merge(end),
                })
            }
            _ => {
                let expr = self.parse_expr()?;
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Expr(expr),
                    span: start.merge(end),
                })
            }
        }
    }

    fn parse_attributed_stmt(&mut self, attrs: Vec<Attribute>, start: Span) -> Result<Stmt, ParseError> {
        match self.kind() {
            TokenKind::KwFunction if self.is_named_function_decl() => {
                let decl = self.parse_function_decl(attrs, start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Function(decl),
                })
            }
            TokenKind::KwClass
            | TokenKind::KwAbstract
            | TokenKind::KwFinal
            | TokenKind::KwReadonly
                if self.is_class_decl_start() =>
            {
                let decl = self.parse_class_decl(attrs, start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Class(decl),
                })
            }
            TokenKind::KwInterface => {
                let decl = self.parse_interface_decl(attrs, start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Interface(decl),
                })
            }
            TokenKind::KwTrait => {
                let decl = self.parse_trait_decl(attrs, start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Trait(decl),
                })
            }
            TokenKind::KwEnum => {
                let decl = self.parse_enum_decl(attrs, start)?;
                Ok(Stmt {
                    span: decl.span,
                    kind: StmtKind::Enum(decl),
                })
            }
            // Attributed closure or arrow function expression statement
            _ => {
                let expr = self.parse_expr_with_attrs(attrs)?;
                let end = self.expect_semi()?;
                Ok(Stmt {
                    kind: StmtKind::Expr(expr),
                    span: start.merge(end),
                })
            }
        }
    }

    pub(crate) fn is_named_function_decl(&self) -> bool {
        if !matches!(self.kind(), TokenKind::KwFunction) {
            return false;
        }
        let next = &self.peek_at(1).kind;
        if matches!(next, TokenKind::Amp) {
            self.peek_at(2).kind.as_member_name().is_some()
        } else {
            next.as_member_name().is_some()
        }
    }

    pub(crate) fn is_class_decl_start(&self) -> bool {
        let mut i = 0;
        loop {
            match &self.peek_at(i).kind {
                TokenKind::KwAbstract | TokenKind::KwFinal | TokenKind::KwReadonly => i += 1,
                TokenKind::KwClass => return true,
                _ => return false,
            }
        }
    }

    pub(crate) fn parse_block_or_stmt(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.eat(&TokenKind::LBrace) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::RBrace, "'}'")?;
            Ok(stmts)
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_if(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // if
        self.expect(&TokenKind::LParen, "'(' after if")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')' after if condition")?;

        if self.eat(&TokenKind::Colon) {
            // Alternative syntax: `if (...): ... elseif (...): ... else: ... endif;`
            let mut then_body = Vec::new();
            while !matches!(
                self.kind(),
                TokenKind::KwElseIf | TokenKind::KwElse | TokenKind::KwEndIf | TokenKind::Eof
            ) {
                then_body.push(self.parse_stmt()?);
            }
            let mut elseifs = Vec::new();
            while self.eat(&TokenKind::KwElseIf) {
                self.expect(&TokenKind::LParen, "'(' after elseif")?;
                let c = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "')' after elseif condition")?;
                self.expect(&TokenKind::Colon, "':' after elseif")?;
                let mut body = Vec::new();
                while !matches!(
                    self.kind(),
                    TokenKind::KwElseIf | TokenKind::KwElse | TokenKind::KwEndIf | TokenKind::Eof
                ) {
                    body.push(self.parse_stmt()?);
                }
                elseifs.push((c, body));
            }
            let else_body = if self.eat(&TokenKind::KwElse) {
                self.expect(&TokenKind::Colon, "':' after else")?;
                let mut body = Vec::new();
                while !matches!(self.kind(), TokenKind::KwEndIf | TokenKind::Eof) {
                    body.push(self.parse_stmt()?);
                }
                Some(body)
            } else {
                None
            };
            self.expect(&TokenKind::KwEndIf, "'endif'")?;
            let end = self.expect_semi()?;
            return Ok(Stmt {
                kind: StmtKind::If {
                    cond,
                    then_body,
                    elseifs,
                    else_body,
                },
                span: start.merge(end),
            });
        }

        let then_body = self.parse_block_or_stmt()?;
        let mut elseifs = Vec::new();
        let mut else_body = None;
        loop {
            if self.eat(&TokenKind::KwElseIf) {
                self.expect(&TokenKind::LParen, "'(' after elseif")?;
                let c = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "')' after elseif condition")?;
                let body = self.parse_block_or_stmt()?;
                elseifs.push((c, body));
            } else if self.eat(&TokenKind::KwElse) {
                if matches!(self.kind(), TokenKind::KwIf) {
                    // `else if (...)` -> treat as elseif
                    self.advance();
                    self.expect(&TokenKind::LParen, "'(' after else if")?;
                    let c = self.parse_expr()?;
                    self.expect(&TokenKind::RParen, "')' after else if condition")?;
                    let body = self.parse_block_or_stmt()?;
                    elseifs.push((c, body));
                } else {
                    else_body = Some(self.parse_block_or_stmt()?);
                    break;
                }
            } else {
                break;
            }
        }
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then_body,
                elseifs,
                else_body,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_while(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // while
        self.expect(&TokenKind::LParen, "'(' after while")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')' after while condition")?;
        let body = if self.eat(&TokenKind::Colon) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::KwEndWhile | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::KwEndWhile, "'endwhile'")?;
            self.expect_semi()?;
            stmts
        } else {
            self.parse_block_or_stmt()?
        };
        Ok(Stmt {
            kind: StmtKind::While { cond, body },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_do_while(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // do
        let body = self.parse_block_or_stmt()?;
        self.expect(&TokenKind::KwWhile, "'while' after do body")?;
        self.expect(&TokenKind::LParen, "'(' after while")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')' after do-while condition")?;
        let end = self.expect_semi()?;
        Ok(Stmt {
            kind: StmtKind::DoWhile { body, cond },
            span: start.merge(end),
        })
    }

    fn parse_for(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // for
        self.expect(&TokenKind::LParen, "'(' after for")?;
        let init = self.parse_expr_list_until(&TokenKind::Semi)?;
        self.expect(&TokenKind::Semi, "';' in for")?;
        let cond = self.parse_expr_list_until(&TokenKind::Semi)?;
        self.expect(&TokenKind::Semi, "';' in for")?;
        let step = self.parse_expr_list_until(&TokenKind::RParen)?;
        self.expect(&TokenKind::RParen, "')' after for clauses")?;
        let body = if self.eat(&TokenKind::Colon) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::KwEndFor | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::KwEndFor, "'endfor'")?;
            self.expect_semi()?;
            stmts
        } else {
            self.parse_block_or_stmt()?
        };
        Ok(Stmt {
            kind: StmtKind::For {
                init,
                cond,
                step,
                body,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_expr_list_until(&mut self, stop: &TokenKind) -> Result<Vec<Expr>, ParseError> {
        let mut out = Vec::new();
        if self.kind() != stop && !self.is_eof() {
            out.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                out.push(self.parse_expr()?);
            }
        }
        Ok(out)
    }

    fn parse_foreach(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // foreach
        self.expect(&TokenKind::LParen, "'(' after foreach")?;
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::KwAs, "'as' in foreach")?;
        let mut by_ref = self.eat(&TokenKind::Amp);
        let first = self.parse_expr()?;
        let (key, value) = if self.eat(&TokenKind::FatArrow) {
            if by_ref {
                return Err(self.error_at(first.span, "foreach key cannot be by-reference"));
            }
            by_ref = self.eat(&TokenKind::Amp);
            let val = self.parse_expr()?;
            (Some(first), val)
        } else {
            (None, first)
        };
        self.expect(&TokenKind::RParen, "')' after foreach")?;
        let body = if self.eat(&TokenKind::Colon) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::KwEndForEach | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::KwEndForEach, "'endforeach'")?;
            self.expect_semi()?;
            stmts
        } else {
            self.parse_block_or_stmt()?
        };
        Ok(Stmt {
            kind: StmtKind::ForEach {
                expr,
                key,
                by_ref,
                value,
                body,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_switch(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // switch
        self.expect(&TokenKind::LParen, "'(' after switch")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')' after switch")?;
        let alt = self.eat(&TokenKind::Colon);
        if !alt {
            self.expect(&TokenKind::LBrace, "'{' after switch")?;
        }
        // PHP allows an optional semicolon right after `{` / `:` before the first `case`.
        self.eat(&TokenKind::Semi);
        let mut cases = Vec::new();
        loop {
            let stop = if alt {
                matches!(self.kind(), TokenKind::KwEndSwitch | TokenKind::Eof)
            } else {
                matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof)
            };
            if stop {
                break;
            }
            let cstart = self.span();
            let ccond = if self.eat(&TokenKind::KwCase) {
                Some(self.parse_expr()?)
            } else if self.eat(&TokenKind::KwDefault) {
                None
            } else {
                return Err(self.error_here("expected 'case' or 'default' in switch"));
            };
            if !self.eat(&TokenKind::Colon) && !self.eat(&TokenKind::Semi) {
                return Err(self.error_here("expected ':' or ';' after switch case"));
            }
            let mut body = Vec::new();
            while !matches!(
                self.kind(),
                TokenKind::KwCase
                    | TokenKind::KwDefault
                    | TokenKind::RBrace
                    | TokenKind::KwEndSwitch
                    | TokenKind::Eof
            ) {
                body.push(self.parse_stmt()?);
            }
            cases.push(SwitchCase {
                cond: ccond,
                body,
                span: cstart.merge(self.prev_span()),
            });
        }
        if alt {
            self.expect(&TokenKind::KwEndSwitch, "'endswitch'")?;
            self.expect_semi()?;
        } else {
            self.expect(&TokenKind::RBrace, "'}' closing switch")?;
        }
        Ok(Stmt {
            kind: StmtKind::Switch { cond, cases },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_try(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // try
        self.expect(&TokenKind::LBrace, "'{' after try")?;
        let mut body = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace, "'}' closing try")?;
        let mut catches = Vec::new();
        while self.eat(&TokenKind::KwCatch) {
            let cstart = self.prev_span();
            self.expect(&TokenKind::LParen, "'(' after catch")?;
            let mut types = vec![self.parse_name()?];
            while self.eat(&TokenKind::Pipe) {
                types.push(self.parse_name()?);
            }
            let var = if matches!(self.kind(), TokenKind::Var(_)) {
                Some(self.expect_var()?.0)
            } else {
                None
            };
            self.expect(&TokenKind::RParen, "')' after catch")?;
            self.expect(&TokenKind::LBrace, "'{' after catch")?;
            let mut cbody = Vec::new();
            while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                cbody.push(self.parse_stmt()?);
            }
            let cend = self.expect(&TokenKind::RBrace, "'}' closing catch")?;
            catches.push(CatchClause {
                types,
                var,
                body: cbody,
                span: cstart.merge(cend),
            });
        }
        let finally = if self.eat(&TokenKind::KwFinally) {
            self.expect(&TokenKind::LBrace, "'{' after finally")?;
            let mut fbody = Vec::new();
            while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                fbody.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::RBrace, "'}' closing finally")?;
            Some(fbody)
        } else {
            None
        };
        if catches.is_empty() && finally.is_none() {
            return Err(self.error_at(start, "try must have at least one catch or finally clause"));
        }
        Ok(Stmt {
            kind: StmtKind::Try {
                body,
                catches,
                finally,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_namespace(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // namespace
        let name = if matches!(self.kind(), TokenKind::LBrace) {
            None
        } else {
            let n = self.parse_name()?;
            Some(n.parts)
        };
        let body = if self.eat(&TokenKind::LBrace) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::RBrace, "'}' closing namespace")?;
            Some(stmts)
        } else {
            self.expect_semi()?;
            None
        };
        Ok(Stmt {
            kind: StmtKind::Namespace { name, body },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_use(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // use
        let outer_kind = if self.eat(&TokenKind::KwFunction) {
            UseKind::Function
        } else if self.eat(&TokenKind::KwConst) {
            UseKind::Const
        } else {
            UseKind::Normal
        };

        let mut items = Vec::new();
        loop {
            let istart = self.span();
            self.eat(&TokenKind::Backslash);
            let mut parts = Vec::new();
            let (first, _) = self.expect_member_name()?;
            parts.push(first);
            while self.eat(&TokenKind::Backslash) {
                if self.eat(&TokenKind::LBrace) {
                    // Group use: `use A\B\{C, function d as e, const F};`
                    while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                        let gstart = self.span();
                        let gkind = if outer_kind != UseKind::Normal {
                            outer_kind
                        } else if self.eat(&TokenKind::KwFunction) {
                            UseKind::Function
                        } else if self.eat(&TokenKind::KwConst) {
                            UseKind::Const
                        } else {
                            UseKind::Normal
                        };
                        let mut full = parts.clone();
                        let (seg, _) = self.expect_member_name()?;
                        full.push(seg);
                        while self.eat(&TokenKind::Backslash) {
                            let (seg2, _) = self.expect_member_name()?;
                            full.push(seg2);
                        }
                        let alias = if self.eat(&TokenKind::KwAs) {
                            Some(self.expect_member_name()?.0)
                        } else {
                            None
                        };
                        items.push(UseItem {
                            kind: gkind,
                            name: full,
                            alias,
                            span: gstart.merge(self.prev_span()),
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RBrace, "'}' closing group use")?;
                    let end = self.expect_semi()?;
                    return Ok(Stmt {
                        kind: StmtKind::Use(items),
                        span: start.merge(end),
                    });
                }
                let (seg, _) = self.expect_member_name()?;
                parts.push(seg);
            }
            let alias = if self.eat(&TokenKind::KwAs) {
                Some(self.expect_member_name()?.0)
            } else {
                None
            };
            items.push(UseItem {
                kind: outer_kind,
                name: parts,
                alias,
                span: istart.merge(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect_semi()?;
        Ok(Stmt {
            kind: StmtKind::Use(items),
            span: start.merge(end),
        })
    }

    fn parse_declare(&mut self, start: Span) -> Result<Stmt, ParseError> {
        self.advance(); // declare
        self.expect(&TokenKind::LParen, "'(' after declare")?;
        let mut directives = Vec::new();
        loop {
            let (name, _) = self.expect_ident()?;
            self.expect(&TokenKind::Eq, "'=' in declare directive")?;
            let val = self.parse_expr()?;
            directives.push((name, val));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')' after declare")?;
        let body = if self.eat(&TokenKind::Colon) {
            let mut stmts = Vec::new();
            while !matches!(self.kind(), TokenKind::KwEndDeclare | TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::KwEndDeclare, "'enddeclare'")?;
            self.expect_semi()?;
            Some(stmts)
        } else if matches!(self.kind(), TokenKind::LBrace) {
            Some(self.parse_block_or_stmt()?)
        } else {
            self.expect_semi()?;
            None
        };
        Ok(Stmt {
            kind: StmtKind::Declare { directives, body },
            span: start.merge(self.prev_span()),
        })
    }
}
