//! Token definitions and source spans for `zipp-rsparse`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[inline]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[inline]
    pub const fn merge(self, other: Span) -> Span {
        Span {
            start: if self.start < other.start { self.start } else { other.start },
            end: if self.end > other.end { self.end } else { other.end },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSuffix {
    None,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSuffix {
    None,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Identifiers & lifetimes
    Ident(String),
    Lifetime(String), // without leading `'`

    // Literals
    Int(u128, IntSuffix),
    Float(f64, FloatSuffix),
    Char(char),
    Byte(u8),
    Str(String),
    ByteStr(Vec<u8>),

    // Strict & common keywords
    As,
    Async,
    Await,
    Break,
    Const,
    Continue,
    Crate,
    Dyn,
    Else,
    Enum,
    Extern,
    False,
    Fn,
    For,
    If,
    Impl,
    In,
    Let,
    Loop,
    Match,
    Mod,
    Move,
    Mut,
    Pub,
    Ref,
    Return,
    SelfLower,
    SelfUpper,
    Static,
    Struct,
    Super,
    Trait,
    True,
    Type,
    Unsafe,
    Use,
    Where,
    While,

    // Punctuation & operators
    LParen,      // (
    RParen,      // )
    LBracket,    // [
    RBracket,    // ]
    LBrace,      // {
    RBrace,      // }
    Comma,       // ,
    Semi,        // ;
    Colon,       // :
    ColonColon,  // ::
    Dot,         // .
    DotDot,      // ..
    DotDotEq,    // ..=
    DotDotDot,   // ...
    Arrow,       // ->
    FatArrow,    // =>
    Pound,       // #
    Dollar,      // $
    Question,    // ?
    At,          // @
    Tilde,       // ~

    Plus,        // +
    Minus,       // -
    Star,        // *
    Slash,       // /
    Percent,     // %
    Caret,       // ^
    Bang,        // !
    Amp,         // &
    Pipe,        // |
    AmpAmp,      // &&
    PipePipe,    // ||
    Shl,         // <<
    Shr,         // >>

    Eq,          // =
    EqEq,        // ==
    NotEq,       // !=
    Lt,          // <
    Le,          // <=
    Gt,          // >
    Ge,          // >=

    PlusEq,      // +=
    MinusEq,     // -=
    StarEq,      // *=
    SlashEq,     // /=
    PercentEq,   // %=
    CaretEq,     // ^=
    AmpEq,       // &=
    PipeEq,      // |=
    ShlEq,       // <<=
    ShrEq,       // >>=

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    #[inline]
    pub fn new(kind: TokenKind, start: u32, end: u32) -> Self {
        Self {
            kind,
            span: Span::new(start, end),
        }
    }
}

pub fn keyword_from_str(s: &str) -> Option<TokenKind> {
    Some(match s {
        "as" => TokenKind::As,
        "async" => TokenKind::Async,
        "await" => TokenKind::Await,
        "break" => TokenKind::Break,
        "const" => TokenKind::Const,
        "continue" => TokenKind::Continue,
        "crate" => TokenKind::Crate,
        "dyn" => TokenKind::Dyn,
        "else" => TokenKind::Else,
        "enum" => TokenKind::Enum,
        "extern" => TokenKind::Extern,
        "false" => TokenKind::False,
        "fn" => TokenKind::Fn,
        "for" => TokenKind::For,
        "if" => TokenKind::If,
        "impl" => TokenKind::Impl,
        "in" => TokenKind::In,
        "let" => TokenKind::Let,
        "loop" => TokenKind::Loop,
        "match" => TokenKind::Match,
        "mod" => TokenKind::Mod,
        "move" => TokenKind::Move,
        "mut" => TokenKind::Mut,
        "pub" => TokenKind::Pub,
        "ref" => TokenKind::Ref,
        "return" => TokenKind::Return,
        "self" => TokenKind::SelfLower,
        "Self" => TokenKind::SelfUpper,
        "static" => TokenKind::Static,
        "struct" => TokenKind::Struct,
        "super" => TokenKind::Super,
        "trait" => TokenKind::Trait,
        "true" => TokenKind::True,
        "type" => TokenKind::Type,
        "unsafe" => TokenKind::Unsafe,
        "use" => TokenKind::Use,
        "where" => TokenKind::Where,
        "while" => TokenKind::While,
        _ => return None,
    })
}
