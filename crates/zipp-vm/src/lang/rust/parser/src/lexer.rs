//! Pure-Rust lexer for `zipp-rsparse`.

use crate::token::{keyword_from_str, FloatSuffix, IntSuffix, Span, Token, TokenKind};
use crate::ParseError;

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    max_tokens: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, max_tokens: usize) -> Self {
        let mut pos = 0;
        // Strip UTF-8 BOM if present.
        if src.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]) {
            pos = 3;
        }
        // Skip shebang `#!...` unless it is an inner attribute `#![...]`.
        if src.as_bytes().get(pos..pos + 2) == Some(b"#!")
            && src.as_bytes().get(pos + 2) != Some(&b'[')
        {
            while pos < src.len() && src.as_bytes()[pos] != b'\n' {
                pos += 1;
            }
        }
        Self {
            src,
            bytes: src.as_bytes(),
            pos,
            max_tokens,
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, ParseError> {
        let mut tokens = Vec::with_capacity((self.src.len() / 6).max(32));
        loop {
            self.skip_ws_and_comments()?;
            if self.pos >= self.bytes.len() {
                tokens.push(Token::new(
                    TokenKind::Eof,
                    self.pos as u32,
                    self.pos as u32,
                ));
                break;
            }
            if tokens.len() >= self.max_tokens {
                return Err(ParseError::new(
                    "token limit exceeded",
                    Span::new(self.pos as u32, self.pos as u32),
                ));
            }
            let tok = self.next_token()?;
            tokens.push(tok);
        }
        Ok(tokens)
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), ParseError> {
        loop {
            while let Some(b) = self.peek() {
                if b.is_ascii_whitespace() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'/') {
                self.pos += 2;
                while let Some(b) = self.peek() {
                    self.pos += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'*') {
                let start = self.pos as u32;
                self.pos += 2;
                let mut depth = 1usize;
                while depth > 0 {
                    if self.pos >= self.bytes.len() {
                        return Err(ParseError::new(
                            "unterminated block comment",
                            Span::new(start, self.pos as u32),
                        ));
                    }
                    if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'*') {
                        depth += 1;
                        self.pos += 2;
                    } else if self.peek() == Some(b'*') && self.peek_at(1) == Some(b'/') {
                        depth -= 1;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        let b = self.bytes[self.pos];

        // Prefixed literals & raw identifiers: r#ident, r"...", r#"..."#, b"...", b'...', br"...", c"...", cr"..."
        if b == b'r' {
            if self.peek_at(1) == Some(b'"')
                || (self.peek_at(1) == Some(b'#') && self.is_raw_string_start(self.pos + 1))
            {
                return self.lex_raw_string(start, false);
            }
            if self.peek_at(1) == Some(b'#')
                && self.peek_at(2).is_some_and(is_ident_start_byte)
            {
                self.pos += 2;
                let id_start = self.pos;
                self.eat_ident_continue();
                let name = self.src[id_start..self.pos].to_owned();
                return Ok(Token::new(
                    TokenKind::Ident(name),
                    start as u32,
                    self.pos as u32,
                ));
            }
        } else if b == b'b' {
            if self.peek_at(1) == Some(b'"') {
                self.pos += 1;
                return self.lex_string(start, true);
            }
            if self.peek_at(1) == Some(b'\'') {
                self.pos += 1;
                return self.lex_byte_char(start);
            }
            if self.peek_at(1) == Some(b'r')
                && (self.peek_at(2) == Some(b'"')
                    || (self.peek_at(2) == Some(b'#')
                        && self.is_raw_string_start(self.pos + 2)))
            {
                self.pos += 1;
                return self.lex_raw_string(start, true);
            }
        } else if b == b'c' {
            if self.peek_at(1) == Some(b'"') {
                self.pos += 1;
                return self.lex_string(start, false);
            }
            if self.peek_at(1) == Some(b'r')
                && (self.peek_at(2) == Some(b'"')
                    || (self.peek_at(2) == Some(b'#')
                        && self.is_raw_string_start(self.pos + 2)))
            {
                self.pos += 1;
                return self.lex_raw_string(start, false);
            }
        }

        // Identifiers and keywords (ASCII + Unicode XID)
        if is_ident_start_byte(b) {
            return Ok(self.lex_ident_or_keyword(start));
        }

        // Numbers
        if b.is_ascii_digit() {
            return self.lex_number(start);
        }

        // Strings
        if b == b'"' {
            return self.lex_string(start, false);
        }

        // Char literals vs lifetimes / loop labels
        if b == b'\'' {
            return self.lex_char_or_lifetime(start);
        }

        // Punctuation & operators
        self.lex_punct(start)
    }

    fn is_raw_string_start(&self, mut i: usize) -> bool {
        while self.bytes.get(i) == Some(&b'#') {
            i += 1;
        }
        self.bytes.get(i) == Some(&b'"')
    }

    fn eat_ident_continue(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else if b >= 0x80 {
                let ch = self.src[self.pos..].chars().next().unwrap();
                if ch.is_alphanumeric() || ch == '_' {
                    self.pos += ch.len_utf8();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn lex_ident_or_keyword(&mut self, start: usize) -> Token {
        self.eat_ident_continue();
        let text = &self.src[start..self.pos];
        let kind = keyword_from_str(text).unwrap_or_else(|| TokenKind::Ident(text.to_owned()));
        Token::new(kind, start as u32, self.pos as u32)
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, ParseError> {
        let mut radix = 10u32;
        if self.bytes[self.pos] == b'0' {
            match self.peek_at(1) {
                Some(b'x' | b'X') => {
                    radix = 16;
                    self.pos += 2;
                }
                Some(b'o' | b'O') => {
                    radix = 8;
                    self.pos += 2;
                }
                Some(b'b' | b'B') => {
                    radix = 2;
                    self.pos += 2;
                }
                _ => {}
            }
        }

        let digits_start = self.pos;
        let mut digits = String::new();
        while let Some(b) = self.peek() {
            if b == b'_' {
                self.pos += 1;
            } else if (radix == 16 && b.is_ascii_hexdigit())
                || (radix == 10 && b.is_ascii_digit())
                || (radix == 8 && (b'0'..=b'7').contains(&b))
                || (radix == 2 && (b == b'0' || b == b'1'))
            {
                digits.push(b as char);
                self.pos += 1;
            } else {
                break;
            }
        }

        // Check for float (only radix 10)
        let mut is_float = false;
        if radix == 10 {
            if self.peek() == Some(b'.') {
                let next = self.peek_at(1);
                // `1.5`, `1.0`, `1.` (not followed by `.` or ident start)
                let dot_is_float = match next {
                    Some(c) if c.is_ascii_digit() => true,
                    Some(b'.') => false,
                    Some(c) if is_ident_start_byte(c) => false,
                    _ => true,
                };
                if dot_is_float {
                    is_float = true;
                    digits.push('.');
                    self.pos += 1;
                    while let Some(b) = self.peek() {
                        if b == b'_' {
                            self.pos += 1;
                        } else if b.is_ascii_digit() {
                            digits.push(b as char);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            if matches!(self.peek(), Some(b'e' | b'E')) {
                is_float = true;
                digits.push('e');
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    digits.push(self.bytes[self.pos] as char);
                    self.pos += 1;
                }
                let exp_start = digits.len();
                while let Some(b) = self.peek() {
                    if b == b'_' {
                        self.pos += 1;
                    } else if b.is_ascii_digit() {
                        digits.push(b as char);
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if digits.len() == exp_start {
                    return Err(ParseError::new(
                        "expected exponent digits in float literal",
                        Span::new(start as u32, self.pos as u32),
                    ));
                }
            }
        }

        // Optional type suffix
        let suf_start = self.pos;
        if self.peek().is_some_and(is_ident_start_byte) {
            self.eat_ident_continue();
        }
        let suffix = &self.src[suf_start..self.pos];

        if suffix == "f32" || suffix == "f64" {
            is_float = true;
        }

        if is_float {
            let fsuf = match suffix {
                "" => FloatSuffix::None,
                "f32" => FloatSuffix::F32,
                "f64" => FloatSuffix::F64,
                _ => {
                    return Err(ParseError::new(
                        format!("invalid float suffix `{suffix}`"),
                        Span::new(start as u32, self.pos as u32),
                    ))
                }
            };
            let clean = if digits.ends_with('.') {
                format!("{digits}0")
            } else {
                digits
            };
            let val = zipp_numparse::parse_f64(&clean).ok_or_else(|| {
                    ParseError::new(
                        "invalid float literal",
                        Span::new(start as u32, self.pos as u32),
                    )
                })?;
            Ok(Token::new(
                TokenKind::Float(val, fsuf),
                start as u32,
                self.pos as u32,
            ))
        } else {
            if digits.is_empty() && self.pos == digits_start {
                return Err(ParseError::new(
                    "missing digits in integer literal",
                    Span::new(start as u32, self.pos as u32),
                ));
            }
            let isuf = match suffix {
                "" => IntSuffix::None,
                "i8" => IntSuffix::I8,
                "i16" => IntSuffix::I16,
                "i32" => IntSuffix::I32,
                "i64" => IntSuffix::I64,
                "i128" => IntSuffix::I128,
                "isize" => IntSuffix::Isize,
                "u8" => IntSuffix::U8,
                "u16" => IntSuffix::U16,
                "u32" => IntSuffix::U32,
                "u64" => IntSuffix::U64,
                "u128" => IntSuffix::U128,
                "usize" => IntSuffix::Usize,
                _ => {
                    return Err(ParseError::new(
                        format!("invalid integer suffix `{suffix}`"),
                        Span::new(start as u32, self.pos as u32),
                    ))
                }
            };
            let val = u128::from_str_radix(&digits, radix).map_err(|_| {
                ParseError::new(
                    "integer literal out of u128 range",
                    Span::new(start as u32, self.pos as u32),
                )
            })?;
            Ok(Token::new(
                TokenKind::Int(val, isuf),
                start as u32,
                self.pos as u32,
            ))
        }
    }

    fn lex_raw_string(&mut self, start: usize, is_byte: bool) -> Result<Token, ParseError> {
        self.pos += 1; // skip 'r'
        let mut hashes = 0usize;
        while self.peek() == Some(b'#') {
            hashes += 1;
            self.pos += 1;
        }
        if self.peek() != Some(b'"') {
            return Err(ParseError::new(
                "expected `\"` to start raw string",
                Span::new(start as u32, self.pos as u32),
            ));
        }
        self.pos += 1; // skip opening '"'
        let content_start = self.pos;
        loop {
            if self.pos >= self.bytes.len() {
                return Err(ParseError::new(
                    "unterminated raw string literal",
                    Span::new(start as u32, self.pos as u32),
                ));
            }
            if self.bytes[self.pos] == b'"' {
                let mut matched = 0usize;
                while matched < hashes && self.peek_at(1 + matched) == Some(b'#') {
                    matched += 1;
                }
                if matched == hashes {
                    let content = &self.src[content_start..self.pos];
                    self.pos += 1 + hashes;
                    let kind = if is_byte {
                        TokenKind::ByteStr(content.as_bytes().to_vec())
                    } else {
                        TokenKind::Str(content.to_owned())
                    };
                    return Ok(Token::new(kind, start as u32, self.pos as u32));
                }
            }
            self.pos += 1;
        }
    }

    fn lex_string(&mut self, start: usize, is_byte: bool) -> Result<Token, ParseError> {
        self.pos += 1; // skip '"'
        let mut out_str = String::new();
        let mut out_bytes = Vec::new();
        loop {
            if self.pos >= self.bytes.len() {
                return Err(ParseError::new(
                    "unterminated string literal",
                    Span::new(start as u32, self.pos as u32),
                ));
            }
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                let kind = if is_byte {
                    TokenKind::ByteStr(out_bytes)
                } else {
                    TokenKind::Str(out_str)
                };
                return Ok(Token::new(kind, start as u32, self.pos as u32));
            }
            if b == b'\\' {
                self.pos += 1;
                if self.peek() == Some(b'\n')
                    || (self.peek() == Some(b'\r') && self.peek_at(1) == Some(b'\n'))
                {
                    // Line continuation escape
                    while let Some(wb) = self.peek() {
                        if wb == b' ' || wb == b'\t' || wb == b'\n' || wb == b'\r' {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    continue;
                }
                if is_byte {
                    let byte = self.lex_byte_escape(start)?;
                    out_bytes.push(byte);
                } else {
                    let ch = self.lex_char_escape(start)?;
                    out_str.push(ch);
                }
                continue;
            }
            if b == b'\r' && self.peek_at(1) == Some(b'\n') {
                self.pos += 2;
                if is_byte {
                    out_bytes.push(b'\n');
                } else {
                    out_str.push('\n');
                }
                continue;
            }
            if is_byte {
                out_bytes.push(b);
                self.pos += 1;
            } else {
                let ch = self.src[self.pos..].chars().next().unwrap();
                out_str.push(ch);
                self.pos += ch.len_utf8();
            }
        }
    }

    fn lex_char_escape(&mut self, start: usize) -> Result<char, ParseError> {
        let Some(esc) = self.peek() else {
            return Err(ParseError::new(
                "unterminated escape sequence",
                Span::new(start as u32, self.pos as u32),
            ));
        };
        self.pos += 1;
        Ok(match esc {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'\\' => '\\',
            b'\'' => '\'',
            b'"' => '"',
            b'0' => '\0',
            b'x' => {
                let h1 = self.read_hex_digit(start)?;
                let h2 = self.read_hex_digit(start)?;
                char::from((h1 << 4) | h2)
            }
            b'u' => {
                if self.peek() != Some(b'{') {
                    return Err(ParseError::new(
                        "expected `{` after `\\u`",
                        Span::new(start as u32, self.pos as u32),
                    ));
                }
                self.pos += 1;
                let mut val = 0u32;
                let mut digits = 0usize;
                while let Some(c) = self.peek() {
                    if c == b'}' {
                        self.pos += 1;
                        break;
                    }
                    if c == b'_' {
                        self.pos += 1;
                        continue;
                    }
                    let d = hex_val(c).ok_or_else(|| {
                        ParseError::new(
                            "invalid unicode escape digit",
                            Span::new(start as u32, self.pos as u32),
                        )
                    })?;
                    val = (val << 4) | d as u32;
                    digits += 1;
                    self.pos += 1;
                }
                if digits == 0 {
                    return Err(ParseError::new(
                        "empty unicode escape",
                        Span::new(start as u32, self.pos as u32),
                    ));
                }
                char::from_u32(val).ok_or_else(|| {
                    ParseError::new(
                        "invalid unicode scalar value",
                        Span::new(start as u32, self.pos as u32),
                    )
                })?
            }
            _ => {
                return Err(ParseError::new(
                    format!("unknown escape `\\{}`", esc as char),
                    Span::new(start as u32, self.pos as u32),
                ))
            }
        })
    }

    fn lex_byte_escape(&mut self, start: usize) -> Result<u8, ParseError> {
        let Some(esc) = self.peek() else {
            return Err(ParseError::new(
                "unterminated byte escape",
                Span::new(start as u32, self.pos as u32),
            ));
        };
        self.pos += 1;
        Ok(match esc {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'\\' => b'\\',
            b'\'' => b'\'',
            b'"' => b'"',
            b'0' => 0,
            b'x' => {
                let h1 = self.read_hex_digit(start)?;
                let h2 = self.read_hex_digit(start)?;
                (h1 << 4) | h2
            }
            _ => {
                return Err(ParseError::new(
                    format!("unknown byte escape `\\{}`", esc as char),
                    Span::new(start as u32, self.pos as u32),
                ))
            }
        })
    }

    fn read_hex_digit(&mut self, start: usize) -> Result<u8, ParseError> {
        let b = self.peek().ok_or_else(|| {
            ParseError::new("expected hex digit", Span::new(start as u32, self.pos as u32))
        })?;
        let v = hex_val(b).ok_or_else(|| {
            ParseError::new("invalid hex digit", Span::new(start as u32, self.pos as u32))
        })?;
        self.pos += 1;
        Ok(v)
    }

    fn lex_byte_char(&mut self, start: usize) -> Result<Token, ParseError> {
        self.pos += 1; // skip '\''
        let byte = if self.peek() == Some(b'\\') {
            self.pos += 1;
            self.lex_byte_escape(start)?
        } else {
            let b = self.peek().ok_or_else(|| {
                ParseError::new(
                    "unterminated byte literal",
                    Span::new(start as u32, self.pos as u32),
                )
            })?;
            self.pos += 1;
            b
        };
        if self.peek() != Some(b'\'') {
            return Err(ParseError::new(
                "expected closing `'` in byte literal",
                Span::new(start as u32, self.pos as u32),
            ));
        }
        self.pos += 1;
        Ok(Token::new(
            TokenKind::Byte(byte),
            start as u32,
            self.pos as u32,
        ))
    }

    fn lex_char_or_lifetime(&mut self, start: usize) -> Result<Token, ParseError> {
        self.pos += 1; // skip '\''
        // Escape char: `'\n'`, `'\''`, `'\u{...}'`
        if self.peek() == Some(b'\\') {
            self.pos += 1;
            let ch = self.lex_char_escape(start)?;
            if self.peek() != Some(b'\'') {
                return Err(ParseError::new(
                    "expected closing `'` in char literal",
                    Span::new(start as u32, self.pos as u32),
                ));
            }
            self.pos += 1;
            return Ok(Token::new(
                TokenKind::Char(ch),
                start as u32,
                self.pos as u32,
            ));
        }
        // Check if single char followed immediately by closing `'`: `'a'`, `'_'`, `'🚀'`
        if let Some(ch) = self.src[self.pos..].chars().next() {
            let next_pos = self.pos + ch.len_utf8();
            if self.bytes.get(next_pos) == Some(&b'\'') && ch != '\'' {
                self.pos = next_pos + 1;
                return Ok(Token::new(
                    TokenKind::Char(ch),
                    start as u32,
                    self.pos as u32,
                ));
            }
        }
        // Otherwise: lifetime or loop label `'a`, `'static`, `'_`
        if self.peek().is_some_and(is_ident_start_byte) {
            let id_start = self.pos;
            self.eat_ident_continue();
            let name = self.src[id_start..self.pos].to_owned();
            return Ok(Token::new(
                TokenKind::Lifetime(name),
                start as u32,
                self.pos as u32,
            ));
        }
        Err(ParseError::new(
            "invalid char literal or lifetime",
            Span::new(start as u32, self.pos as u32),
        ))
    }

    fn lex_punct(&mut self, start: usize) -> Result<Token, ParseError> {
        let b0 = self.bytes[self.pos];
        let b1 = self.peek_at(1);
        let b2 = self.peek_at(2);

        // 3-char operators
        match (b0, b1, b2) {
            (b'.', Some(b'.'), Some(b'=')) => {
                self.pos += 3;
                return Ok(Token::new(TokenKind::DotDotEq, start as u32, self.pos as u32));
            }
            (b'.', Some(b'.'), Some(b'.')) => {
                self.pos += 3;
                return Ok(Token::new(TokenKind::DotDotDot, start as u32, self.pos as u32));
            }
            (b'<', Some(b'<'), Some(b'=')) => {
                self.pos += 3;
                return Ok(Token::new(TokenKind::ShlEq, start as u32, self.pos as u32));
            }
            (b'>', Some(b'>'), Some(b'=')) => {
                self.pos += 3;
                return Ok(Token::new(TokenKind::ShrEq, start as u32, self.pos as u32));
            }
            _ => {}
        }

        // 2-char operators
        let two = match (b0, b1) {
            (b':', Some(b':')) => Some(TokenKind::ColonColon),
            (b'.', Some(b'.')) => Some(TokenKind::DotDot),
            (b'-', Some(b'>')) => Some(TokenKind::Arrow),
            (b'=', Some(b'>')) => Some(TokenKind::FatArrow),
            (b'=', Some(b'=')) => Some(TokenKind::EqEq),
            (b'!', Some(b'=')) => Some(TokenKind::NotEq),
            (b'<', Some(b'=')) => Some(TokenKind::Le),
            (b'>', Some(b'=')) => Some(TokenKind::Ge),
            (b'<', Some(b'<')) => Some(TokenKind::Shl),
            (b'>', Some(b'>')) => Some(TokenKind::Shr),
            (b'&', Some(b'&')) => Some(TokenKind::AmpAmp),
            (b'|', Some(b'|')) => Some(TokenKind::PipePipe),
            (b'+', Some(b'=')) => Some(TokenKind::PlusEq),
            (b'-', Some(b'=')) => Some(TokenKind::MinusEq),
            (b'*', Some(b'=')) => Some(TokenKind::StarEq),
            (b'/', Some(b'=')) => Some(TokenKind::SlashEq),
            (b'%', Some(b'=')) => Some(TokenKind::PercentEq),
            (b'^', Some(b'=')) => Some(TokenKind::CaretEq),
            (b'&', Some(b'=')) => Some(TokenKind::AmpEq),
            (b'|', Some(b'=')) => Some(TokenKind::PipeEq),
            _ => None,
        };
        if let Some(kind) = two {
            self.pos += 2;
            return Ok(Token::new(kind, start as u32, self.pos as u32));
        }

        // 1-char tokens
        let one = match b0 {
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b',' => TokenKind::Comma,
            b';' => TokenKind::Semi,
            b':' => TokenKind::Colon,
            b'.' => TokenKind::Dot,
            b'#' => TokenKind::Pound,
            b'$' => TokenKind::Dollar,
            b'?' => TokenKind::Question,
            b'@' => TokenKind::At,
            b'~' => TokenKind::Tilde,
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'^' => TokenKind::Caret,
            b'!' => TokenKind::Bang,
            b'&' => TokenKind::Amp,
            b'|' => TokenKind::Pipe,
            b'=' => TokenKind::Eq,
            b'<' => TokenKind::Lt,
            b'>' => TokenKind::Gt,
            _ => {
                return Err(ParseError::new(
                    format!("unexpected character `{}`", b0 as char),
                    Span::new(start as u32, (start + 1) as u32),
                ));
            }
        };
        self.pos += 1;
        Ok(Token::new(one, start as u32, self.pos as u32))
    }
}

#[inline]
fn is_ident_start_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
