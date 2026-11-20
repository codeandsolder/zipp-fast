//! AST definitions for `zipp-rsparse`.

use crate::token::{FloatSuffix, IntSuffix, Span, Token};

#[derive(Debug, Clone)]
pub struct Crate {
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub is_inner: bool,
    pub path: Vec<String>,
    /// Raw tokens inside `(...)` or `= expr` if present.
    pub args: AttrArgs,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AttrArgs {
    Empty,
    Delimited(Vec<Token>),
    Eq(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
    PubCrate,
    PubSuper,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    Fn(FnItem),
    Struct(StructItem),
    Enum(EnumItem),
    Impl(ImplItem),
    Trait(TraitItem),
    Const(ConstItem),
    Static(StaticItem),
    TypeAlias(TypeAliasItem),
    Mod(ModItem),
    Use(UseTree),
    MacroCall(MacroCall),
    MacroRules(MacroRulesItem),
    ExternCrate(String, Option<String>),
}

#[derive(Debug, Clone)]
pub struct FnItem {
    pub name: String,
    pub is_async: bool,
    pub is_const: bool,
    pub is_unsafe: bool,
    pub params: Vec<FnParam>,
    pub ret_type: Option<Type>,
    pub body: Option<Block>,
}

#[derive(Debug, Clone)]
pub struct FnParam {
    pub is_self: bool,
    pub is_mut_self: bool,
    pub is_ref_self: bool,
    pub pat: Pat,
    pub ty: Option<Type>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructItem {
    pub name: String,
    pub fields: StructFields,
}

#[derive(Debug, Clone)]
pub enum StructFields {
    Named(Vec<FieldDef>),
    Tuple(Vec<TupleFieldDef>),
    Unit,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TupleFieldDef {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumItem {
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub fields: StructFields,
    pub discriminant: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplItem {
    pub trait_path: Option<Path>,
    pub self_type: Type,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct TraitItem {
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct ConstItem {
    pub name: String,
    pub ty: Type,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct StaticItem {
    pub name: String,
    pub is_mut: bool,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct TypeAliasItem {
    pub name: String,
    pub target: Option<Type>,
}

#[derive(Debug, Clone)]
pub struct ModItem {
    pub name: String,
    /// `None` for `mod foo;` (external file), `Some(items)` for `mod foo { ... }`.
    pub items: Option<Vec<Item>>,
}

#[derive(Debug, Clone)]
pub struct UseTree {
    pub prefix: Vec<String>,
    pub kind: UseTreeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum UseTreeKind {
    Simple(Option<String>), // optional `as alias`
    Glob,
    Group(Vec<UseTree>),
}

#[derive(Debug, Clone)]
pub struct MacroRulesItem {
    pub name: String,
    pub rules: Vec<MacroRule>,
}

#[derive(Debug, Clone)]
pub struct MacroRule {
    pub matcher: Vec<Token>,
    pub body: Vec<Token>,
}

#[derive(Debug, Clone)]
pub struct MacroCall {
    pub path: Path,
    pub delim: MacroDelim,
    pub tokens: Vec<Token>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroDelim {
    Paren,
    Bracket,
    Brace,
}

#[derive(Debug, Clone)]
pub struct Path {
    pub leading_colon: bool,
    pub segments: Vec<PathSegment>,
    pub span: Span,
}

impl Path {
    pub fn simple(name: impl Into<String>, span: Span) -> Self {
        Self {
            leading_colon: false,
            segments: vec![PathSegment {
                ident: name.into(),
                type_args: Vec::new(),
            }],
            span,
        }
    }

    pub fn last_ident(&self) -> &str {
        self.segments.last().map(|s| s.ident.as_str()).unwrap_or("")
    }

    pub fn as_names(&self) -> Vec<&str> {
        self.segments.iter().map(|s| s.ident.as_str()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct PathSegment {
    pub ident: String,
    pub type_args: Vec<Type>,
}

#[derive(Debug, Clone)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Path(Path),
    Tuple(Vec<Type>),
    Array(Box<Type>, Option<Box<Expr>>),
    Slice(Box<Type>),
    Ref { is_mut: bool, inner: Box<Type> },
    Ptr { is_mut: bool, inner: Box<Type> },
    FnPtr { params: Vec<Type>, ret: Option<Box<Type>> },
    ImplTrait(Vec<Path>),
    DynTrait(Vec<Path>),
    Never,
    Infer,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub attrs: Vec<Attribute>,
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        pat: Pat,
        ty: Option<Type>,
        init: Option<Expr>,
        else_block: Option<Block>,
    },
    Item(Box<Item>),
    Expr {
        expr: Expr,
        has_semi: bool,
    },
    Empty,
}

#[derive(Debug, Clone)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatKind {
    Wildcard,
    Rest,
    Ident {
        is_mut: bool,
        is_ref: bool,
        name: String,
        subpat: Option<Box<Pat>>,
    },
    Literal(Literal),
    NegLiteral(Literal),
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    Tuple(Vec<Pat>),
    Slice(Vec<Pat>),
    Path(Path),
    TupleStruct {
        path: Path,
        elems: Vec<Pat>,
    },
    Struct {
        path: Path,
        fields: Vec<FieldPat>,
        has_rest: bool,
    },
    Or(Vec<Pat>),
    Ref {
        is_mut: bool,
        inner: Box<Pat>,
    },
}

#[derive(Debug, Clone)]
pub struct FieldPat {
    pub name: String,
    pub pat: Pat,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Literal {
    Int(u128, IntSuffix),
    Float(f64, FloatSuffix),
    Bool(bool),
    Char(char),
    Byte(u8),
    Str(String),
    ByteStr(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Literal(Literal),
    Path(Path),
    QualifiedPath {
        ty: Type,
        as_trait: Option<Path>,
        item: String,
    },
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    Repeat {
        elem: Box<Expr>,
        count: Box<Expr>,
    },
    Struct {
        path: Path,
        fields: Vec<FieldInit>,
        rest: Option<Box<Expr>>,
    },
    Block(Block),
    UnsafeBlock(Block),
    AsyncBlock {
        is_move: bool,
        block: Block,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Ref {
        is_mut: bool,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: Option<BinOp>,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    Try(Box<Expr>),
    Await(Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        type_args: Vec<Type>,
        args: Vec<Expr>,
    },
    Field {
        receiver: Box<Expr>,
        field: String,
    },
    TupleIndex {
        receiver: Box<Expr>,
        index: usize,
    },
    Index {
        receiver: Box<Expr>,
        index: Box<Expr>,
    },
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    If {
        cond: Box<IfCond>,
        then_block: Block,
        else_branch: Option<Box<Expr>>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Loop {
        label: Option<String>,
        body: Block,
    },
    While {
        label: Option<String>,
        cond: Box<IfCond>,
        body: Block,
    },
    For {
        label: Option<String>,
        pat: Pat,
        iter: Box<Expr>,
        body: Block,
    },
    Break {
        label: Option<String>,
        value: Option<Box<Expr>>,
    },
    Continue {
        label: Option<String>,
    },
    Return(Option<Box<Expr>>),
    Closure {
        is_async: bool,
        is_move: bool,
        params: Vec<(Pat, Option<Type>)>,
        ret_type: Option<Type>,
        body: Box<Expr>,
    },
    MacroCall(MacroCall),
}

#[derive(Debug, Clone)]
pub enum IfCond {
    Expr(Expr),
    Let { pat: Pat, expr: Expr },
    /// Let-chains: `if let Some(a) = x && a > 0 && let Some(b) = y`
    Chain(Vec<IfCond>),
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub attrs: Vec<Attribute>,
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
