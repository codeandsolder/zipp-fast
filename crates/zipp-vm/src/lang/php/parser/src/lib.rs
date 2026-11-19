//! `zipp-phpparse`: ZIPP's own pure-Rust PHP 8.x lexer and parser.
//!
//! Zero `unsafe` code and zero external dependencies so it compiles identically
//! into the native JIT CLI, the hardened `safe-sandbox` runner, and `wasm32`.
#![forbid(unsafe_code)]

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

use std::fmt;

/// Compile-time resource bounds enforced during lexing and parsing.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_tokens: usize,
    pub max_depth: usize,
}

impl Limits {
    pub const DEFAULT: Self = Self {
        max_tokens: 1 << 20,
        max_depth: 192,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A syntax error with byte offset and 1-based line/column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: u32,
    pub line: u32,
    pub col: u32,
}

impl ParseError {
    pub fn new(source: &str, offset: u32, message: impl Into<String>) -> Self {
        let (line, col) = line_col(source, offset as usize);
        Self {
            message: message.into(),
            offset,
            line,
            col,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Parse error: {} on line {} (col {})",
            self.message, self.line, self.col
        )
    }
}

impl std::error::Error for ParseError {}

/// Compute 1-based `(line, col)` for a byte offset in `source`.
pub fn line_col(source: &str, offset: usize) -> (u32, u32) {
    let clamped = offset.min(source.len());
    let mut line = 1u32;
    let mut line_start = 0usize;
    for (i, &b) in source.as_bytes()[..clamped].iter().enumerate() {
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let col = (clamped - line_start) as u32 + 1;
    (line, col)
}

/// Parse a PHP source file (starting in inline HTML mode) into an [`ast::Program`].
pub fn parse(source: &str, limits: Limits) -> Result<ast::Program, ParseError> {
    let tokens = lexer::lex(source, limits)?;
    parser::parse_tokens(source, &tokens, limits)
}
