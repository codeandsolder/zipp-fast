//! PHP 8.x token definitions.

/// Byte range `[start, end)` in the original source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn merge(self, other: Span) -> Self {
        Self {
            start: if self.start < other.start { self.start } else { other.start },
            end: if self.end > other.end { self.end } else { other.end },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// One segment of an interpolated double-quoted string or heredoc.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpChunk {
    /// Decoded raw byte-string literal segment (stored as Latin-1 chars U+0000..=U+00FF).
    Literal(String),
    /// Embedded PHP source expression text (lexed/parsed by the parser with accurate spans).
    ExprSource { source: String, offset: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Raw bytes outside `<?php ... ?>` (stored as byte-string: each byte -> char U+0000..=U+00FF).
    InlineHtml(String),
    /// `<?=` short echo tag (transitions into PHP mode and emits echo expression).
    OpenEchoTag,
    /// `?>` closing PHP tag (acts as implicit semicolon in statements).
    CloseTag,

    // ---- Literals ----
    IntLit(i64),
    FloatLit(f64),
    /// Single-quoted, nowdoc, or escape-decoded non-interpolated double-quoted string.
    /// Stored as PHP byte-string (Latin-1 code units U+0000..=U+00FF).
    StringLit(String),
    /// Interpolated double-quoted string or heredoc.
    InterpString(Vec<InterpChunk>),

    // ---- Identifiers & Variables ----
    /// Bare identifier (class, function, constant, namespace segment, etc.) preserving original case.
    Ident(String),
    /// `$name` (stored without the leading `$`).
    Var(String),

    // ---- Keywords (case-insensitive in PHP) ----
    KwAbstract,
    KwAnd,
    KwArray,
    KwAs,
    KwBreak,
    KwCallable,
    KwCase,
    KwCatch,
    KwClass,
    KwClone,
    KwConst,
    KwContinue,
    KwDeclare,
    KwDefault,
    KwDie,
    KwDo,
    KwEcho,
    KwElse,
    KwElseIf,
    KwEmpty,
    KwEndDeclare,
    KwEndFor,
    KwEndForEach,
    KwEndIf,
    KwEndSwitch,
    KwEndWhile,
    KwEnum,
    KwEval,
    KwExit,
    KwExtends,
    KwFinal,
    KwFinally,
    KwFn,
    KwFor,
    KwForEach,
    KwFrom,
    KwFunction,
    KwGlobal,
    KwGoto,
    KwIf,
    KwImplements,
    KwInclude,
    KwIncludeOnce,
    KwInstanceOf,
    KwInsteadOf,
    KwInterface,
    KwIsset,
    KwList,
    KwMatch,
    KwNamespace,
    KwNew,
    KwOr,
    KwPrint,
    KwPrivate,
    KwProtected,
    KwPublic,
    KwReadonly,
    KwRequire,
    KwRequireOnce,
    KwReturn,
    KwStatic,
    KwSwitch,
    KwThrow,
    KwTrait,
    KwTry,
    KwUnset,
    KwUse,
    KwVar,
    KwWhile,
    KwXor,
    KwYield,
    KwTrue,
    KwFalse,
    KwNull,
    // Magic constants
    MagicFile,
    MagicDir,
    MagicLine,
    MagicFunction,
    MagicClass,
    MagicMethod,
    MagicNamespace,
    MagicTrait,

    // ---- Punctuation & Operators ----
    LParen,        // (
    RParen,        // )
    LBracket,      // [
    RBracket,      // ]
    LBrace,        // {
    RBrace,        // }
    Semi,          // ;
    Comma,         // ,
    Colon,         // :
    DoubleColon,   // ::
    Question,      // ?
    DoubleQuestion,// ??
    QuestionAssign,// ??=
    NullsafeArrow, // ?->
    Arrow,         // ->
    FatArrow,      // =>
    Ellipsis,      // ...
    Backslash,     // \
    At,            // @
    Dollar,        // $ (for $$x or ${expr})
    AttrOpen,      // #[

    Plus,          // +
    Minus,         // -
    Star,          // *
    StarStar,      // **
    Slash,         // /
    Percent,       // %
    Dot,           // .
    PlusPlus,      // ++
    MinusMinus,    // --

    Eq,            // =
    EqEq,          // ==
    EqEqEq,        // ===
    BangEq,        // != or <>
    BangEqEq,      // !==
    Lt,            // <
    LtEq,          // <=
    Gt,            // >
    GtEq,          // >=
    Spaceship,     // <=>

    PlusEq,        // +=
    MinusEq,       // -=
    StarEq,        // *=
    StarStarEq,    // **=
    SlashEq,       // /=
    PercentEq,     // %=
    DotEq,         // .=
    AmpEq,         // &=
    PipeEq,        // |=
    CaretEq,       // ^=
    ShlEq,         // <<=
    ShrEq,         // >>=

    Bang,          // !
    Tilde,         // ~
    Amp,           // &
    AmpAmp,        // &&
    Pipe,          // |
    PipePipe,      // ||
    Caret,         // ^
    Shl,           // <<
    Shr,           // >>

    Eof,
}

impl TokenKind {
    /// If this token is an identifier or a keyword that PHP allows as a
    /// property, method, class constant, or namespace name, return its spelling.
    pub fn as_member_name(&self) -> Option<&str> {
        Some(match self {
            TokenKind::Ident(s) => s.as_str(),
            TokenKind::KwAbstract => "abstract",
            TokenKind::KwAnd => "and",
            TokenKind::KwArray => "array",
            TokenKind::KwAs => "as",
            TokenKind::KwBreak => "break",
            TokenKind::KwCallable => "callable",
            TokenKind::KwCase => "case",
            TokenKind::KwCatch => "catch",
            TokenKind::KwClass => "class",
            TokenKind::KwClone => "clone",
            TokenKind::KwConst => "const",
            TokenKind::KwContinue => "continue",
            TokenKind::KwDeclare => "declare",
            TokenKind::KwDefault => "default",
            TokenKind::KwDie => "die",
            TokenKind::KwDo => "do",
            TokenKind::KwEcho => "echo",
            TokenKind::KwElse => "else",
            TokenKind::KwElseIf => "elseif",
            TokenKind::KwEmpty => "empty",
            TokenKind::KwEndDeclare => "enddeclare",
            TokenKind::KwEndFor => "endfor",
            TokenKind::KwEndForEach => "endforeach",
            TokenKind::KwEndIf => "endif",
            TokenKind::KwEndSwitch => "endswitch",
            TokenKind::KwEndWhile => "endwhile",
            TokenKind::KwEnum => "enum",
            TokenKind::KwEval => "eval",
            TokenKind::KwExit => "exit",
            TokenKind::KwExtends => "extends",
            TokenKind::KwFinal => "final",
            TokenKind::KwFinally => "finally",
            TokenKind::KwFn => "fn",
            TokenKind::KwFor => "for",
            TokenKind::KwForEach => "foreach",
            TokenKind::KwFrom => "from",
            TokenKind::KwFunction => "function",
            TokenKind::KwGlobal => "global",
            TokenKind::KwGoto => "goto",
            TokenKind::KwIf => "if",
            TokenKind::KwImplements => "implements",
            TokenKind::KwInclude => "include",
            TokenKind::KwIncludeOnce => "include_once",
            TokenKind::KwInstanceOf => "instanceof",
            TokenKind::KwInsteadOf => "insteadof",
            TokenKind::KwInterface => "interface",
            TokenKind::KwIsset => "isset",
            TokenKind::KwList => "list",
            TokenKind::KwMatch => "match",
            TokenKind::KwNamespace => "namespace",
            TokenKind::KwNew => "new",
            TokenKind::KwOr => "or",
            TokenKind::KwPrint => "print",
            TokenKind::KwPrivate => "private",
            TokenKind::KwProtected => "protected",
            TokenKind::KwPublic => "public",
            TokenKind::KwReadonly => "readonly",
            TokenKind::KwRequire => "require",
            TokenKind::KwRequireOnce => "require_once",
            TokenKind::KwReturn => "return",
            TokenKind::KwStatic => "static",
            TokenKind::KwSwitch => "switch",
            TokenKind::KwThrow => "throw",
            TokenKind::KwTrait => "trait",
            TokenKind::KwTry => "try",
            TokenKind::KwUnset => "unset",
            TokenKind::KwUse => "use",
            TokenKind::KwVar => "var",
            TokenKind::KwWhile => "while",
            TokenKind::KwXor => "xor",
            TokenKind::KwYield => "yield",
            TokenKind::KwTrue => "true",
            TokenKind::KwFalse => "false",
            TokenKind::KwNull => "null",
            _ => return None,
        })
    }

    pub fn lookup_keyword(s: &str) -> Option<TokenKind> {
        // Fast length/case-insensitive match.
        Some(match s.to_ascii_lowercase().as_str() {
            "abstract" => TokenKind::KwAbstract,
            "and" => TokenKind::KwAnd,
            "array" => TokenKind::KwArray,
            "as" => TokenKind::KwAs,
            "break" => TokenKind::KwBreak,
            "callable" => TokenKind::KwCallable,
            "case" => TokenKind::KwCase,
            "catch" => TokenKind::KwCatch,
            "class" => TokenKind::KwClass,
            "clone" => TokenKind::KwClone,
            "const" => TokenKind::KwConst,
            "continue" => TokenKind::KwContinue,
            "declare" => TokenKind::KwDeclare,
            "default" => TokenKind::KwDefault,
            "die" => TokenKind::KwDie,
            "do" => TokenKind::KwDo,
            "echo" => TokenKind::KwEcho,
            "else" => TokenKind::KwElse,
            "elseif" => TokenKind::KwElseIf,
            "empty" => TokenKind::KwEmpty,
            "enddeclare" => TokenKind::KwEndDeclare,
            "endfor" => TokenKind::KwEndFor,
            "endforeach" => TokenKind::KwEndForEach,
            "endif" => TokenKind::KwEndIf,
            "endswitch" => TokenKind::KwEndSwitch,
            "endwhile" => TokenKind::KwEndWhile,
            "enum" => TokenKind::KwEnum,
            "eval" => TokenKind::KwEval,
            "exit" => TokenKind::KwExit,
            "extends" => TokenKind::KwExtends,
            "final" => TokenKind::KwFinal,
            "finally" => TokenKind::KwFinally,
            "fn" => TokenKind::KwFn,
            "for" => TokenKind::KwFor,
            "foreach" => TokenKind::KwForEach,
            "from" => TokenKind::KwFrom,
            "function" => TokenKind::KwFunction,
            "global" => TokenKind::KwGlobal,
            "goto" => TokenKind::KwGoto,
            "if" => TokenKind::KwIf,
            "implements" => TokenKind::KwImplements,
            "include" => TokenKind::KwInclude,
            "include_once" => TokenKind::KwIncludeOnce,
            "instanceof" => TokenKind::KwInstanceOf,
            "insteadof" => TokenKind::KwInsteadOf,
            "interface" => TokenKind::KwInterface,
            "isset" => TokenKind::KwIsset,
            "list" => TokenKind::KwList,
            "match" => TokenKind::KwMatch,
            "namespace" => TokenKind::KwNamespace,
            "new" => TokenKind::KwNew,
            "or" => TokenKind::KwOr,
            "print" => TokenKind::KwPrint,
            "private" => TokenKind::KwPrivate,
            "protected" => TokenKind::KwProtected,
            "public" => TokenKind::KwPublic,
            "readonly" => TokenKind::KwReadonly,
            "require" => TokenKind::KwRequire,
            "require_once" => TokenKind::KwRequireOnce,
            "return" => TokenKind::KwReturn,
            "static" => TokenKind::KwStatic,
            "switch" => TokenKind::KwSwitch,
            "throw" => TokenKind::KwThrow,
            "trait" => TokenKind::KwTrait,
            "try" => TokenKind::KwTry,
            "unset" => TokenKind::KwUnset,
            "use" => TokenKind::KwUse,
            "var" => TokenKind::KwVar,
            "while" => TokenKind::KwWhile,
            "xor" => TokenKind::KwXor,
            "yield" => TokenKind::KwYield,
            "true" => TokenKind::KwTrue,
            "false" => TokenKind::KwFalse,
            "null" => TokenKind::KwNull,
            "__file__" => TokenKind::MagicFile,
            "__dir__" => TokenKind::MagicDir,
            "__line__" => TokenKind::MagicLine,
            "__function__" => TokenKind::MagicFunction,
            "__class__" => TokenKind::MagicClass,
            "__method__" => TokenKind::MagicMethod,
            "__namespace__" => TokenKind::MagicNamespace,
            "__trait__" => TokenKind::MagicTrait,
            _ => return None,
        })
    }
}
