//! `zipp-rsparse` — ZIPP's pure-Rust (`#![forbid(unsafe_code)]`) Rust lexer and parser.

#![forbid(unsafe_code)]

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

use std::fmt;
use token::Span;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_tokens: usize,
    pub max_depth: usize,
}

impl Limits {
    pub const DEFAULT: Self = Self {
        max_tokens: 1_000_000,
        max_depth: 128,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn format_with_source(&self, source: &str, filename: &str) -> String {
        let off = (self.span.start as usize).min(source.len());
        let mut line = 1usize;
        let mut col = 1usize;
        for &b in &source.as_bytes()[..off] {
            if b == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        format!("{filename}:{line}:{col}: parse error: {}", self.message)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at byte {}: {}", self.span.start, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse a Rust source file into an [`ast::Crate`].
pub fn parse(source: &str, limits: Limits) -> Result<ast::Crate, ParseError> {
    let tokens = lexer::Lexer::new(source, limits.max_tokens).tokenize()?;
    let mut p = parser::Parser::new(source, tokens, limits);
    p.parse_crate()
}

/// Parse a Rust expression from a slice of tokens (used by macro expansion).
pub fn parse_expr_tokens(
    source: &str,
    mut tokens: Vec<token::Token>,
    limits: Limits,
) -> Result<ast::Expr, ParseError> {
    if !matches!(tokens.last().map(|t| &t.kind), Some(token::TokenKind::Eof)) {
        let end = tokens.last().map(|t| t.span.end).unwrap_or(0);
        tokens.push(token::Token::new(token::TokenKind::Eof, end, end));
    }
    let mut p = parser::Parser::new(source, tokens, limits);
    let expr = p.parse_expr()?;
    Ok(expr)
}

/// Parse a sequence of Rust statements/items from tokens (used by macro expansion).
pub fn parse_stmts_tokens(
    source: &str,
    mut tokens: Vec<token::Token>,
    limits: Limits,
) -> Result<Vec<ast::Stmt>, ParseError> {
    if !matches!(tokens.last().map(|t| &t.kind), Some(token::TokenKind::Eof)) {
        let end = tokens.last().map(|t| t.span.end).unwrap_or(0);
        tokens.push(token::Token::new(token::TokenKind::Eof, end, end));
    }
    let mut p = parser::Parser::new(source, tokens, limits);
    p.parse_stmt_list_until_eof()
}
