//! Dual-mode PHP 8.x lexer (Inline HTML + PHP code + string interpolation + heredoc/nowdoc).

use crate::token::{InterpChunk, Span, Token, TokenKind};
use crate::{Limits, ParseError};

/// Push raw bytes into a PHP byte-string (Latin-1 code units U+0000..=U+00FF).
#[inline]
fn push_raw_bytes(out: &mut String, bytes: &[u8]) {
    for &b in bytes {
        out.push(char::from(b));
    }
}

/// Lex PHP source starting in inline HTML mode.
pub fn lex(source: &str, limits: Limits) -> Result<Vec<Token>, ParseError> {
    lex_inner(source, 0, false, limits)
}

/// Lex an embedded PHP expression slice (already in PHP code mode, offset-adjusted).
pub fn lex_php_expr(source: &str, base_offset: u32, limits: Limits) -> Result<Vec<Token>, ParseError> {
    lex_inner(source, base_offset, true, limits)
}

fn lex_inner(
    source: &str,
    base_offset: u32,
    start_in_php: bool,
    limits: Limits,
) -> Result<Vec<Token>, ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;
    let mut in_php = start_in_php;
    let mut tokens: Vec<Token> = Vec::with_capacity(len / 6 + 8);

    // Strip UTF-8 BOM at file start (when not in embedded expr).
    if !start_in_php && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        pos = 3;
    }
    // Strip leading `#!` shebang in CLI scripts (PHP ignores line 1 `#!...`).
    if !start_in_php && bytes[pos..].starts_with(b"#!") {
        while pos < len && bytes[pos] != b'\n' {
            pos += 1;
        }
        if pos < len && bytes[pos] == b'\n' {
            pos += 1;
        }
    }

    let push_tok = |tokens: &mut Vec<Token>, kind: TokenKind, start: usize, end: usize| -> Result<(), ParseError> {
        if tokens.len() >= limits.max_tokens {
            return Err(ParseError::new(
                source,
                base_offset + start as u32,
                "token limit exceeded",
            ));
        }
        tokens.push(Token {
            kind,
            span: Span::new(base_offset + start as u32, base_offset + end as u32),
        });
        Ok(())
    };

    while pos < len {
        if !in_php {
            let html_start = pos;
            let mut found_open: Option<(usize, bool)> = None; // (end_of_tag, is_echo)
            while pos < len {
                if bytes[pos] == b'<' && pos + 1 < len && bytes[pos + 1] == b'?' {
                    if pos + 2 < len && bytes[pos + 2] == b'=' {
                        found_open = Some((pos + 3, true));
                        break;
                    }
                    if pos + 5 <= len
                        && bytes[pos + 2..pos + 5].eq_ignore_ascii_case(b"php")
                        && (pos + 5 == len
                            || matches!(bytes[pos + 5], b' ' | b'\t' | b'\r' | b'\n'))
                    {
                        let mut tag_end = pos + 5;
                        // Consume single space/newline after `<?php` per PHP lexer.
                        if tag_end < len {
                            if bytes[tag_end] == b'\r' && tag_end + 1 < len && bytes[tag_end + 1] == b'\n' {
                                tag_end += 2;
                            } else if matches!(bytes[tag_end], b' ' | b'\t' | b'\r' | b'\n') {
                                tag_end += 1;
                            }
                        }
                        found_open = Some((tag_end, false));
                        break;
                    }
                }
                pos += 1;
            }
            if pos > html_start {
                let mut html = String::with_capacity(pos - html_start);
                push_raw_bytes(&mut html, &bytes[html_start..pos]);
                push_tok(&mut tokens, TokenKind::InlineHtml(html), html_start, pos)?;
            }
            if let Some((tag_end, is_echo)) = found_open {
                let tag_start = pos;
                pos = tag_end;
                in_php = true;
                if is_echo {
                    push_tok(&mut tokens, TokenKind::OpenEchoTag, tag_start, tag_end)?;
                }
            }
            continue;
        }

        // ---- PHP code mode ----
        // Skip whitespace.
        while pos < len && matches!(bytes[pos], b' ' | b'\t' | b'\r' | b'\n' | 0x0C) {
            pos += 1;
        }
        if pos >= len {
            break;
        }

        let start = pos;
        let b = bytes[pos];

        // Closing tag `?>`
        if !start_in_php && b == b'?' && pos + 1 < len && bytes[pos + 1] == b'>' {
            pos += 2;
            // PHP consumes one trailing newline after `?>`.
            if pos < len && bytes[pos] == b'\r' {
                pos += 1;
                if pos < len && bytes[pos] == b'\n' {
                    pos += 1;
                }
            } else if pos < len && bytes[pos] == b'\n' {
                pos += 1;
            }
            push_tok(&mut tokens, TokenKind::CloseTag, start, pos)?;
            in_php = false;
            continue;
        }

        // Line comments: `//...` or `#...` (not `#[` attribute!)
        if (b == b'/' && pos + 1 < len && bytes[pos + 1] == b'/')
            || (b == b'#' && !(pos + 1 < len && bytes[pos + 1] == b'['))
        {
            pos += if b == b'/' { 2 } else { 1 };
            while pos < len {
                if bytes[pos] == b'\n' || bytes[pos] == b'\r' {
                    break;
                }
                if !start_in_php && bytes[pos] == b'?' && pos + 1 < len && bytes[pos + 1] == b'>' {
                    break;
                }
                pos += 1;
            }
            continue;
        }

        // Block comments: `/* ... */`
        if b == b'/' && pos + 1 < len && bytes[pos + 1] == b'*' {
            pos += 2;
            let mut closed = false;
            while pos + 1 < len {
                if bytes[pos] == b'*' && bytes[pos + 1] == b'/' {
                    pos += 2;
                    closed = true;
                    break;
                }
                pos += 1;
            }
            if !closed {
                return Err(ParseError::new(
                    source,
                    base_offset + start as u32,
                    "unterminated block comment",
                ));
            }
            continue;
        }

        // PHP 8 attribute open `#[`
        if b == b'#' && pos + 1 < len && bytes[pos + 1] == b'[' {
            pos += 2;
            push_tok(&mut tokens, TokenKind::AttrOpen, start, pos)?;
            continue;
        }

        // Binary string prefix `b"..."`, `b'...'`, `B"..."`, `B'...'`
        if matches!(b, b'b' | b'B')
            && pos + 1 < len
            && matches!(bytes[pos + 1], b'\'' | b'"')
        {
            pos += 1;
            let q = bytes[pos];
            if q == b'\'' {
                let (s, end) = lex_single_quoted(source, base_offset, pos)?;
                pos = end;
                push_tok(&mut tokens, TokenKind::StringLit(s), start, pos)?;
            } else {
                let (tok, end) = lex_double_quoted(source, base_offset, pos)?;
                pos = end;
                push_tok(&mut tokens, tok, start, pos)?;
            }
            continue;
        }

        // Single-quoted string `'...'`
        if b == b'\'' {
            let (s, end) = lex_single_quoted(source, base_offset, pos)?;
            pos = end;
            push_tok(&mut tokens, TokenKind::StringLit(s), start, pos)?;
            continue;
        }

        // Double-quoted string `"..."`
        if b == b'"' {
            let (tok, end) = lex_double_quoted(source, base_offset, pos)?;
            pos = end;
            push_tok(&mut tokens, tok, start, pos)?;
            continue;
        }

        // Heredoc / Nowdoc: `<<<`
        if b == b'<' && pos + 2 < len && bytes[pos + 1] == b'<' && bytes[pos + 2] == b'<' {
            let (tok, end) = lex_heredoc(source, base_offset, pos)?;
            pos = end;
            push_tok(&mut tokens, tok, start, pos)?;
            continue;
        }

        // Variable `$ident` or standalone `$` (for `$$x`, `${expr}`)
        if b == b'$' {
            if pos + 1 < len && is_ident_start(bytes[pos + 1]) {
                pos += 1;
                let name_start = pos;
                while pos < len && is_ident_continue(bytes[pos]) {
                    pos += 1;
                }
                let name = source[name_start..pos].to_owned();
                push_tok(&mut tokens, TokenKind::Var(name), start, pos)?;
            } else {
                pos += 1;
                push_tok(&mut tokens, TokenKind::Dollar, start, pos)?;
            }
            continue;
        }

        // Numbers: decimal, hex, bin, oct, float (`.5` handled below)
        if b.is_ascii_digit()
            || (b == b'.' && pos + 1 < len && bytes[pos + 1].is_ascii_digit())
        {
            let (tok, end) = lex_number(source, base_offset, pos)?;
            pos = end;
            push_tok(&mut tokens, tok, start, pos)?;
            continue;
        }

        // Identifiers and keywords
        if is_ident_start(b) {
            pos += 1;
            while pos < len && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            let text = &source[start..pos];
            let kind = TokenKind::lookup_keyword(text)
                .unwrap_or_else(|| TokenKind::Ident(text.to_owned()));
            push_tok(&mut tokens, kind, start, pos)?;
            continue;
        }

        // Multi-char and single-char operators / punctuation
        let rem = &bytes[pos..];
        let (kind, width) = if rem.starts_with(b"===") {
            (TokenKind::EqEqEq, 3)
        } else if rem.starts_with(b"!==") {
            (TokenKind::BangEqEq, 3)
        } else if rem.starts_with(b"<=>") {
            (TokenKind::Spaceship, 3)
        } else if rem.starts_with(b"...") {
            (TokenKind::Ellipsis, 3)
        } else if rem.starts_with(b"?->") {
            (TokenKind::NullsafeArrow, 3)
        } else if rem.starts_with(b"??=") {
            (TokenKind::QuestionAssign, 3)
        } else if rem.starts_with(b"**=") {
            (TokenKind::StarStarEq, 3)
        } else if rem.starts_with(b"<<=") {
            (TokenKind::ShlEq, 3)
        } else if rem.starts_with(b">>=") {
            (TokenKind::ShrEq, 3)
        } else if rem.starts_with(b"==") {
            (TokenKind::EqEq, 2)
        } else if rem.starts_with(b"!=") || rem.starts_with(b"<>") {
            (TokenKind::BangEq, 2)
        } else if rem.starts_with(b"<=") {
            (TokenKind::LtEq, 2)
        } else if rem.starts_with(b">=") {
            (TokenKind::GtEq, 2)
        } else if rem.starts_with(b"=>") {
            (TokenKind::FatArrow, 2)
        } else if rem.starts_with(b"->") {
            (TokenKind::Arrow, 2)
        } else if rem.starts_with(b"::") {
            (TokenKind::DoubleColon, 2)
        } else if rem.starts_with(b"??") {
            (TokenKind::DoubleQuestion, 2)
        } else if rem.starts_with(b"**") {
            (TokenKind::StarStar, 2)
        } else if rem.starts_with(b"++") {
            (TokenKind::PlusPlus, 2)
        } else if rem.starts_with(b"--") {
            (TokenKind::MinusMinus, 2)
        } else if rem.starts_with(b"&&") {
            (TokenKind::AmpAmp, 2)
        } else if rem.starts_with(b"||") {
            (TokenKind::PipePipe, 2)
        } else if rem.starts_with(b"<<") {
            (TokenKind::Shl, 2)
        } else if rem.starts_with(b">>") {
            (TokenKind::Shr, 2)
        } else if rem.starts_with(b"+=") {
            (TokenKind::PlusEq, 2)
        } else if rem.starts_with(b"-=") {
            (TokenKind::MinusEq, 2)
        } else if rem.starts_with(b"*=") {
            (TokenKind::StarEq, 2)
        } else if rem.starts_with(b"/=") {
            (TokenKind::SlashEq, 2)
        } else if rem.starts_with(b"%=") {
            (TokenKind::PercentEq, 2)
        } else if rem.starts_with(b".=") {
            (TokenKind::DotEq, 2)
        } else if rem.starts_with(b"&=") {
            (TokenKind::AmpEq, 2)
        } else if rem.starts_with(b"|=") {
            (TokenKind::PipeEq, 2)
        } else if rem.starts_with(b"^=") {
            (TokenKind::CaretEq, 2)
        } else {
            match b {
                b'(' => (TokenKind::LParen, 1),
                b')' => (TokenKind::RParen, 1),
                b'[' => (TokenKind::LBracket, 1),
                b']' => (TokenKind::RBracket, 1),
                b'{' => (TokenKind::LBrace, 1),
                b'}' => (TokenKind::RBrace, 1),
                b';' => (TokenKind::Semi, 1),
                b',' => (TokenKind::Comma, 1),
                b':' => (TokenKind::Colon, 1),
                b'?' => (TokenKind::Question, 1),
                b'\\' => (TokenKind::Backslash, 1),
                b'@' => (TokenKind::At, 1),
                b'+' => (TokenKind::Plus, 1),
                b'-' => (TokenKind::Minus, 1),
                b'*' => (TokenKind::Star, 1),
                b'/' => (TokenKind::Slash, 1),
                b'%' => (TokenKind::Percent, 1),
                b'.' => (TokenKind::Dot, 1),
                b'=' => (TokenKind::Eq, 1),
                b'<' => (TokenKind::Lt, 1),
                b'>' => (TokenKind::Gt, 1),
                b'!' => (TokenKind::Bang, 1),
                b'~' => (TokenKind::Tilde, 1),
                b'&' => (TokenKind::Amp, 1),
                b'|' => (TokenKind::Pipe, 1),
                b'^' => (TokenKind::Caret, 1),
                _ => {
                    return Err(ParseError::new(
                        source,
                        base_offset + start as u32,
                        format!("unexpected character {:?}", b as char),
                    ));
                }
            }
        };
        pos += width;
        push_tok(&mut tokens, kind, start, pos)?;
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(base_offset + len as u32, base_offset + len as u32),
    });
    Ok(tokens)
}

#[inline]
pub(crate) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

#[inline]
pub(crate) fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn lex_single_quoted(source: &str, base_offset: u32, start: usize) -> Result<(String, usize), ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = start + 1;
    let mut out = String::new();
    while pos < len {
        let b = bytes[pos];
        if b == b'\'' {
            return Ok((out, pos + 1));
        }
        if b == b'\\' && pos + 1 < len {
            let next = bytes[pos + 1];
            if next == b'\'' || next == b'\\' {
                out.push(char::from(next));
                pos += 2;
                continue;
            }
        }
        out.push(char::from(b));
        pos += 1;
    }
    Err(ParseError::new(
        source,
        base_offset + start as u32,
        "unterminated single-quoted string",
    ))
}

fn lex_double_quoted(source: &str, base_offset: u32, start: usize) -> Result<(TokenKind, usize), ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = start + 1;
    let mut chunks: Vec<InterpChunk> = Vec::new();
    let mut lit = String::new();
    let mut has_interp = false;

    while pos < len {
        let b = bytes[pos];
        if b == b'"' {
            pos += 1;
            if !has_interp {
                return Ok((TokenKind::StringLit(lit), pos));
            }
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(lit));
            }
            return Ok((TokenKind::InterpString(chunks), pos));
        }
        if b == b'\\' && pos + 1 < len {
            pos = decode_escape(bytes, pos, &mut lit, true);
            continue;
        }
        // Complex interpolation `{$...}`
        if b == b'{' && pos + 1 < len && bytes[pos + 1] == b'$' {
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(std::mem::take(&mut lit)));
            }
            has_interp = true;
            let expr_start = pos + 1;
            let close = scan_balanced_brace(source, base_offset, pos)?;
            let expr_src = source[expr_start..close].to_owned();
            chunks.push(InterpChunk::ExprSource {
                source: expr_src,
                offset: base_offset + expr_start as u32,
            });
            pos = close + 1;
            continue;
        }
        // `${...}` interpolation
        if b == b'$' && pos + 1 < len && bytes[pos + 1] == b'{' {
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(std::mem::take(&mut lit)));
            }
            has_interp = true;
            let inner_start = pos + 2;
            let close = scan_balanced_brace(source, base_offset, pos + 1)?;
            let inner = source[inner_start..close].trim();
            let expr_src = if inner.bytes().all(is_ident_continue)
                && inner.bytes().next().is_some_and(is_ident_start)
            {
                format!("${inner}")
            } else {
                format!("${{{inner}}}")
            };
            chunks.push(InterpChunk::ExprSource {
                source: expr_src,
                offset: base_offset + inner_start as u32,
            });
            pos = close + 1;
            continue;
        }
        // Simple interpolation `$var`, `$var[key]`, `$var->prop`, `$var?->prop`
        if b == b'$' && pos + 1 < len && is_ident_start(bytes[pos + 1]) {
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(std::mem::take(&mut lit)));
            }
            has_interp = true;
            let (expr_src, end) = scan_simple_interp(source, pos);
            chunks.push(InterpChunk::ExprSource {
                source: expr_src,
                offset: base_offset + pos as u32,
            });
            pos = end;
            continue;
        }
        lit.push(char::from(b));
        pos += 1;
    }

    Err(ParseError::new(
        source,
        base_offset + start as u32,
        "unterminated double-quoted string",
    ))
}

/// Scan `{$...}` or `${...}` matching braces while respecting nested quotes.
fn scan_balanced_brace(source: &str, base_offset: u32, open_pos: usize) -> Result<usize, ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = open_pos + 1;
    let mut depth = 1usize;
    while pos < len {
        let b = bytes[pos];
        match b {
            b'\'' => {
                pos += 1;
                while pos < len {
                    if bytes[pos] == b'\\' && pos + 1 < len {
                        pos += 2;
                    } else if bytes[pos] == b'\'' {
                        pos += 1;
                        break;
                    } else {
                        pos += 1;
                    }
                }
            }
            b'"' => {
                pos += 1;
                while pos < len {
                    if bytes[pos] == b'\\' && pos + 1 < len {
                        pos += 2;
                    } else if bytes[pos] == b'"' {
                        pos += 1;
                        break;
                    } else {
                        pos += 1;
                    }
                }
            }
            b'{' => {
                depth += 1;
                pos += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(pos);
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }
    Err(ParseError::new(
        source,
        base_offset + open_pos as u32,
        "unterminated '{' in interpolated string",
    ))
}

/// Scan simple string variable interpolation: `$name`, `$name[idx]`, `$name->prop`, `$name?->prop`.
fn scan_simple_interp(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = start + 1;
    while pos < len && is_ident_continue(bytes[pos]) {
        pos += 1;
    }
    let var = &source[start..pos];

    // Check `->prop` or `?->prop`
    if pos + 2 < len && bytes[pos] == b'-' && bytes[pos + 1] == b'>' && is_ident_start(bytes[pos + 2]) {
        let pstart = pos + 2;
        let mut pend = pstart + 1;
        while pend < len && is_ident_continue(bytes[pend]) {
            pend += 1;
        }
        return (format!("{var}->{}", &source[pstart..pend]), pend);
    }
    if pos + 3 < len
        && bytes[pos] == b'?'
        && bytes[pos + 1] == b'-'
        && bytes[pos + 2] == b'>'
        && is_ident_start(bytes[pos + 3])
    {
        let pstart = pos + 3;
        let mut pend = pstart + 1;
        while pend < len && is_ident_continue(bytes[pend]) {
            pend += 1;
        }
        return (format!("{var}?->{}", &source[pstart..pend]), pend);
    }
    // Check `[key]` (only one bracket level in PHP simple syntax!)
    if pos < len && bytes[pos] == b'[' {
        let mut kpos = pos + 1;
        // `$arr[$k]`
        if kpos + 1 < len && bytes[kpos] == b'$' && is_ident_start(bytes[kpos + 1]) {
            let ks = kpos;
            kpos += 2;
            while kpos < len && is_ident_continue(bytes[kpos]) {
                kpos += 1;
            }
            if kpos < len && bytes[kpos] == b']' {
                return (format!("{var}[{}]", &source[ks..kpos]), kpos + 1);
            }
        }
        // `$arr[123]` or `$arr[-1]`
        let ks = kpos;
        if kpos < len && bytes[kpos] == b'-' {
            kpos += 1;
        }
        if kpos < len && bytes[kpos].is_ascii_digit() {
            while kpos < len && bytes[kpos].is_ascii_digit() {
                kpos += 1;
            }
            if kpos < len && bytes[kpos] == b']' {
                return (format!("{var}[{}]", &source[ks..kpos]), kpos + 1);
            }
        }
        // `$arr[unquoted_key]` -> `$arr['unquoted_key']`
        kpos = pos + 1;
        if kpos < len && is_ident_start(bytes[kpos]) {
            let ks = kpos;
            while kpos < len && is_ident_continue(bytes[kpos]) {
                kpos += 1;
            }
            if kpos < len && bytes[kpos] == b']' {
                return (format!("{var}['{}']", &source[ks..kpos]), kpos + 1);
            }
        }
    }

    (var.to_owned(), pos)
}

/// Decode one backslash escape inside a double-quoted string or heredoc.
fn decode_escape(bytes: &[u8], pos: usize, out: &mut String, escape_quote: bool) -> usize {
    let len = bytes.len();
    let next = bytes[pos + 1];
    match next {
        b'n' => {
            out.push('\n');
            pos + 2
        }
        b'r' => {
            out.push('\r');
            pos + 2
        }
        b't' => {
            out.push('\t');
            pos + 2
        }
        b'v' => {
            out.push('\x0B');
            pos + 2
        }
        b'e' => {
            out.push('\x1B');
            pos + 2
        }
        b'f' => {
            out.push('\x0C');
            pos + 2
        }
        b'\\' => {
            out.push('\\');
            pos + 2
        }
        b'$' => {
            out.push('$');
            pos + 2
        }
        b'"' if escape_quote => {
            out.push('"');
            pos + 2
        }
        b'x' => {
            let mut v = 0u8;
            let mut count = 0usize;
            let mut p = pos + 2;
            while count < 2 && p < len {
                if let Some(d) = hex_val(bytes[p]) {
                    v = (v << 4) | d;
                    p += 1;
                    count += 1;
                } else {
                    break;
                }
            }
            if count > 0 {
                out.push(char::from(v));
                p
            } else {
                out.push('\\');
                out.push('x');
                pos + 2
            }
        }
        b'u' if pos + 2 < len && bytes[pos + 2] == b'{' => {
            let mut p = pos + 3;
            let mut cp = 0u32;
            let mut count = 0usize;
            while p < len && bytes[p] != b'}' {
                if let Some(d) = hex_val(bytes[p]) {
                    cp = cp.saturating_mul(16).saturating_add(d as u32);
                    p += 1;
                    count += 1;
                } else {
                    break;
                }
            }
            if count > 0 && p < len && bytes[p] == b'}' {
                if let Some(c) = char::from_u32(cp) {
                    let mut buf = [0u8; 4];
                    let encoded = c.encode_utf8(&mut buf);
                    push_raw_bytes(out, encoded.as_bytes());
                    return p + 1;
                }
            }
            out.push('\\');
            out.push('u');
            pos + 2
        }
        b'0'..=b'7' => {
            let mut v = 0u32;
            let mut count = 0usize;
            let mut p = pos + 1;
            while count < 3 && p < len && matches!(bytes[p], b'0'..=b'7') {
                v = (v << 3) | (bytes[p] - b'0') as u32;
                p += 1;
                count += 1;
            }
            out.push(char::from((v & 0xFF) as u8));
            p
        }
        _ => {
            out.push('\\');
            out.push(char::from(next));
            pos + 2
        }
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Lex PHP 7.3+ flexible heredoc (`<<<ID` / `<<<"ID"`) and nowdoc (`<<<'ID'`).
fn lex_heredoc(source: &str, base_offset: u32, start: usize) -> Result<(TokenKind, usize), ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = start + 3;
    while pos < len && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }
    let quote = if pos < len && matches!(bytes[pos], b'\'' | b'"') {
        let q = bytes[pos];
        pos += 1;
        Some(q)
    } else {
        None
    };
    let is_nowdoc = quote == Some(b'\'');
    let label_start = pos;
    if pos >= len || !is_ident_start(bytes[pos]) {
        return Err(ParseError::new(
            source,
            base_offset + start as u32,
            "invalid heredoc/nowdoc label",
        ));
    }
    while pos < len && is_ident_continue(bytes[pos]) {
        pos += 1;
    }
    let label = &source[label_start..pos];
    if let Some(q) = quote {
        if pos >= len || bytes[pos] != q {
            return Err(ParseError::new(
                source,
                base_offset + start as u32,
                "unclosed quote in heredoc/nowdoc label",
            ));
        }
        pos += 1;
    }
    if pos < len && bytes[pos] == b'\r' {
        pos += 1;
    }
    if pos >= len || bytes[pos] != b'\n' {
        return Err(ParseError::new(
            source,
            base_offset + pos as u32,
            "expected newline after heredoc/nowdoc label",
        ));
    }
    pos += 1;

    let body_start = pos;
    // Scan lines until we find `[spaces/tabs]LABEL` followed by non-ident char.
    let mut end_line_start = None;
    let mut close_end = pos;
    let mut indent = String::new();
    let mut line_pos = pos;
    while line_pos <= len {
        let mut p = line_pos;
        while p < len && matches!(bytes[p], b' ' | b'\t') {
            p += 1;
        }
        if bytes[p..].starts_with(label.as_bytes()) {
            let after = p + label.len();
            if after == len || !is_ident_continue(bytes[after]) {
                end_line_start = Some(line_pos);
                indent = source[line_pos..p].to_owned();
                close_end = after;
                break;
            }
        }
        while line_pos < len && bytes[line_pos] != b'\n' {
            line_pos += 1;
        }
        if line_pos >= len {
            break;
        }
        line_pos += 1;
    }

    let Some(body_end_with_nl) = end_line_start else {
        return Err(ParseError::new(
            source,
            base_offset + start as u32,
            format!("unterminated heredoc/nowdoc `{label}`"),
        ));
    };

    // Strip the single trailing newline immediately preceding the closing marker.
    let mut body_end = body_end_with_nl;
    if body_end > body_start && bytes[body_end - 1] == b'\n' {
        body_end -= 1;
        if body_end > body_start && bytes[body_end - 1] == b'\r' {
            body_end -= 1;
        }
    }

    let raw_body = &source[body_start..body_end];
    // Strip flexible indentation prefix from each line.
    let stripped = if indent.is_empty() || raw_body.is_empty() {
        raw_body.to_owned()
    } else {
        let mut out = String::with_capacity(raw_body.len());
        let mut first = true;
        for line in raw_body.split('\n') {
            if !first {
                out.push('\n');
            }
            first = false;
            if let Some(rest) = line.strip_prefix(&indent) {
                out.push_str(rest);
            } else if line.trim_end_matches('\r').is_empty() {
                if line.ends_with('\r') {
                    out.push('\r');
                }
            } else {
                out.push_str(line);
            }
        }
        out
    };

    if is_nowdoc {
        let mut out = String::with_capacity(stripped.len());
        push_raw_bytes(&mut out, stripped.as_bytes());
        return Ok((TokenKind::StringLit(out), close_end));
    }

    // Parse heredoc body with double-quote escape & interpolation rules (without `\"`).
    let hb = stripped.as_bytes();
    let hlen = hb.len();
    let mut hp = 0usize;
    let mut chunks = Vec::new();
    let mut lit = String::new();
    let mut has_interp = false;
    while hp < hlen {
        let b = hb[hp];
        if b == b'\\' && hp + 1 < hlen {
            hp = decode_escape(hb, hp, &mut lit, false);
            continue;
        }
        if b == b'{' && hp + 1 < hlen && hb[hp + 1] == b'$' {
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(std::mem::take(&mut lit)));
            }
            has_interp = true;
            let close = scan_balanced_brace(&stripped, base_offset + body_start as u32, hp)?;
            chunks.push(InterpChunk::ExprSource {
                source: stripped[hp + 1..close].to_owned(),
                offset: base_offset + (body_start + hp + 1) as u32,
            });
            hp = close + 1;
            continue;
        }
        if b == b'$' && hp + 1 < hlen && is_ident_start(hb[hp + 1]) {
            if !lit.is_empty() {
                chunks.push(InterpChunk::Literal(std::mem::take(&mut lit)));
            }
            has_interp = true;
            let (expr_src, end) = scan_simple_interp(&stripped, hp);
            chunks.push(InterpChunk::ExprSource {
                source: expr_src,
                offset: base_offset + (body_start + hp) as u32,
            });
            hp = end;
            continue;
        }
        lit.push(char::from(b));
        hp += 1;
    }

    if !has_interp {
        Ok((TokenKind::StringLit(lit), close_end))
    } else {
        if !lit.is_empty() {
            chunks.push(InterpChunk::Literal(lit));
        }
        Ok((TokenKind::InterpString(chunks), close_end))
    }
}

fn lex_number(source: &str, base_offset: u32, start: usize) -> Result<(TokenKind, usize), ParseError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut pos = start;

    // Radix prefixes: 0x / 0b / 0o
    if bytes[pos] == b'0' && pos + 1 < len {
        let pfx = bytes[pos + 1];
        if matches!(pfx, b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
            let radix: u32 = match pfx {
                b'x' | b'X' => 16,
                b'b' | b'B' => 2,
                _ => 8,
            };
            pos += 2;
            let mut clean = String::new();
            while pos < len {
                let c = bytes[pos];
                if c == b'_' {
                    pos += 1;
                    continue;
                }
                let ok = match radix {
                    16 => c.is_ascii_hexdigit(),
                    2 => matches!(c, b'0' | b'1'),
                    _ => matches!(c, b'0'..=b'7'),
                };
                if ok {
                    clean.push(c as char);
                    pos += 1;
                } else {
                    break;
                }
            }
            if clean.is_empty() {
                return Err(ParseError::new(
                    source,
                    base_offset + start as u32,
                    "invalid numeric literal",
                ));
            }
            if let Ok(v) = i64::from_str_radix(&clean, radix) {
                return Ok((TokenKind::IntLit(v), pos));
            }
            // Overflow promotes to float in PHP.
            let mut f = 0.0f64;
            for ch in clean.bytes() {
                let d = hex_val(ch).unwrap_or(0) as f64;
                f = f * radix as f64 + d;
            }
            return Ok((TokenKind::FloatLit(f), pos));
        }
    }

    let mut is_float = false;
    let mut clean = String::new();
    while pos < len {
        let c = bytes[pos];
        if c.is_ascii_digit() {
            clean.push(c as char);
            pos += 1;
        } else if c == b'_' && pos + 1 < len && bytes[pos + 1].is_ascii_digit() {
            pos += 1;
        } else {
            break;
        }
    }
    if pos < len && bytes[pos] == b'.' && !(pos + 1 < len && bytes[pos + 1] == b'.') {
        // In PHP, `1.foo` is concat `1 . foo`, whereas `1.` or `1.5` is float ONLY if not followed by ident?
        // Actually in PHP `1.` IS float 1.0 unless it's `1..2`, and `1.5` is float 1.5!
        // Note: in PHP `1 . $x` requires space or non-digit, `1.a` is actually float `1.` + syntax error,
        // wait: in PHP `echo 1.2;` is 1.2, `echo 1 . 2;` is "12". Checking digit or not-ident handles both cleanly.
        if pos + 1 < len && bytes[pos + 1].is_ascii_digit() || !clean.is_empty() && (pos + 1 == len || !is_ident_start(bytes[pos + 1])) {
            is_float = true;
            clean.push('.');
            pos += 1;
            while pos < len {
                let c = bytes[pos];
                if c.is_ascii_digit() {
                    clean.push(c as char);
                    pos += 1;
                } else if c == b'_' && pos + 1 < len && bytes[pos + 1].is_ascii_digit() {
                    pos += 1;
                } else {
                    break;
                }
            }
        }
    }
    if pos < len && matches!(bytes[pos], b'e' | b'E') {
        let save = pos;
        let mut exp = String::new();
        exp.push('e');
        pos += 1;
        if pos < len && matches!(bytes[pos], b'+' | b'-') {
            exp.push(bytes[pos] as char);
            pos += 1;
        }
        let dstart = pos;
        while pos < len {
            let c = bytes[pos];
            if c.is_ascii_digit() {
                exp.push(c as char);
                pos += 1;
            } else if c == b'_' && pos + 1 < len && bytes[pos + 1].is_ascii_digit() {
                pos += 1;
            } else {
                break;
            }
        }
        if pos > dstart {
            is_float = true;
            clean.push_str(&exp);
        } else {
            pos = save;
        }
    }

    if is_float {
        let f = zipp_numparse::parse_f64(&clean).ok_or_else(|| {
            ParseError::new(source, base_offset + start as u32, "invalid float literal")
        })?;
        return Ok((TokenKind::FloatLit(f), pos));
    }

    // Legacy octal literal `0755` (leading `0` with more digits)
    if clean.len() > 1 && clean.starts_with('0') {
        if let Ok(v) = i64::from_str_radix(&clean, 8) {
            return Ok((TokenKind::IntLit(v), pos));
        }
        let mut f = 0.0f64;
        for ch in clean.bytes() {
            if matches!(ch, b'0'..=b'7') {
                f = f * 8.0 + (ch - b'0') as f64;
            }
        }
        return Ok((TokenKind::FloatLit(f), pos));
    }

    if let Ok(v) = clean.parse::<i64>() {
        Ok((TokenKind::IntLit(v), pos))
    } else {
        let f = zipp_numparse::parse_f64(&clean).ok_or_else(|| {
            ParseError::new(source, base_offset + start as u32, "invalid numeric literal")
        })?;
        Ok((TokenKind::FloatLit(f), pos))
    }
}
