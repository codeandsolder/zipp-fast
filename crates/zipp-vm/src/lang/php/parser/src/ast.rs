//! PHP 8.x Abstract Syntax Tree.

use crate::token::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

/// Qualified, relative, or unqualified PHP symbol name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub kind: NameKind,
    pub parts: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// `Foo` or `Foo\Bar` (relative to current namespace / `use` imports).
    Unqualified,
    /// `\Foo\Bar` (fully qualified from global root).
    FullyQualified,
    /// `namespace\Foo\Bar` (explicitly relative to current namespace).
    Relative,
}

impl Name {
    pub fn joined(&self) -> String {
        self.parts.join("\\")
    }
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Name,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeHint {
    Simple(Name),
    Nullable(Box<TypeHint>),
    Union(Vec<TypeHint>),
    Intersection(Vec<TypeHint>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub attributes: Vec<Attribute>,
    pub visibility: Option<Visibility>,
    pub set_visibility: Option<Visibility>,
    pub readonly: bool,
    pub ty: Option<TypeHint>,
    pub by_ref: bool,
    pub variadic: bool,
    pub name: String,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<String>,
    pub unpack: bool,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ClosureUse {
    pub name: String,
    pub by_ref: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    Normal,
    Function,
    Const,
}

#[derive(Debug, Clone)]
pub struct UseItem {
    pub kind: UseKind,
    pub name: Vec<String>,
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    InlineHtml(String),
    Nop,
    Expr(Expr),
    Block(Vec<Stmt>),
    Echo(Vec<Expr>),
    Return(Option<Expr>),
    Break(Option<Expr>),
    Continue(Option<Expr>),
    Global(Vec<String>),
    StaticVar(Vec<(String, Option<Expr>)>),
    Unset(Vec<Expr>),
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        elseifs: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    DoWhile {
        body: Vec<Stmt>,
        cond: Expr,
    },
    For {
        init: Vec<Expr>,
        cond: Vec<Expr>,
        step: Vec<Expr>,
        body: Vec<Stmt>,
    },
    ForEach {
        expr: Expr,
        key: Option<Expr>,
        by_ref: bool,
        value: Expr,
        body: Vec<Stmt>,
    },
    Switch {
        cond: Expr,
        cases: Vec<SwitchCase>,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<CatchClause>,
        finally: Option<Vec<Stmt>>,
    },
    Function(FunctionDecl),
    Class(ClassDecl),
    Interface(InterfaceDecl),
    Trait(TraitDecl),
    Enum(EnumDecl),
    Namespace {
        name: Option<Vec<String>>,
        /// `Some(stmts)` for `namespace Foo { ... }`, `None` for `namespace Foo;`.
        body: Option<Vec<Stmt>>,
    },
    Use(Vec<UseItem>),
    Const(Vec<(String, Expr)>),
    Declare {
        directives: Vec<(String, Expr)>,
        body: Option<Vec<Stmt>>,
    },
    Label(String),
    Goto(String),
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// `None` for `default:`.
    pub cond: Option<Expr>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CatchClause {
    pub types: Vec<Name>,
    pub var: Option<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub by_ref: bool,
    pub params: Vec<Param>,
    pub return_type: Option<TypeHint>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub attributes: Vec<Attribute>,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
    pub name: String,
    pub extends: Option<Name>,
    pub implements: Vec<Name>,
    pub members: Vec<ClassMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct InterfaceDecl {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub extends: Vec<Name>,
    pub members: Vec<ClassMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub members: Vec<ClassMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub backing_type: Option<String>,
    pub implements: Vec<Name>,
    pub cases: Vec<EnumCase>,
    pub members: Vec<ClassMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumCase {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropHookKind {
    Get,
    Set,
}

#[derive(Debug, Clone)]
pub enum PropHookBody {
    Expr(Expr),
    Block(Vec<Stmt>),
    Abstract,
}

#[derive(Debug, Clone)]
pub struct PropertyHook {
    pub kind: PropHookKind,
    pub by_ref: bool,
    pub param: Option<Param>,
    pub body: PropHookBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ClassMember {
    Property {
        attributes: Vec<Attribute>,
        visibility: Visibility,
        set_visibility: Option<Visibility>,
        is_static: bool,
        is_readonly: bool,
        ty: Option<TypeHint>,
        items: Vec<(String, Option<Expr>)>,
        hooks: Vec<PropertyHook>,
        span: Span,
    },
    Method {
        attributes: Vec<Attribute>,
        visibility: Visibility,
        is_static: bool,
        is_abstract: bool,
        is_final: bool,
        by_ref: bool,
        name: String,
        params: Vec<Param>,
        return_type: Option<TypeHint>,
        body: Option<Vec<Stmt>>,
        span: Span,
    },
    Const {
        attributes: Vec<Attribute>,
        visibility: Visibility,
        is_final: bool,
        ty: Option<TypeHint>,
        items: Vec<(String, Expr)>,
        span: Span,
    },
    TraitUse {
        traits: Vec<Name>,
        adaptations: Vec<TraitAdaptation>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum TraitAdaptation {
    Precedence {
        trait_name: Option<Name>,
        method: String,
        instead_of: Vec<Name>,
    },
    Alias {
        trait_name: Option<Name>,
        method: String,
        visibility: Option<Visibility>,
        alias: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub by_ref: bool,
    pub unpack: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    /// `None` for `default => ...`.
    pub conds: Option<Vec<Expr>>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicConst {
    File,
    Dir,
    Line,
    Function,
    Class,
    Method,
    Namespace,
    Trait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Neg,
    Not,
    BitNot,
    Suppress, // @
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Spaceship,
    LogAnd,
    LogOr,
    LogXor,
    Coalesce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AssignRef,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Coalesce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    Int,
    Float,
    String,
    Bool,
    Array,
    Object,
    Unset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeKind {
    Include,
    IncludeOnce,
    Require,
    RequireOnce,
}

#[derive(Debug, Clone)]
pub enum CallArgs {
    Normal(Vec<Arg>),
    /// PHP 8.1 first-class callable syntax: `foo(...)`, `$o->m(...)`.
    FirstClassCallable,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    InterpString(Vec<StringPart>),
    MagicConst(MagicConst),
    Var(String),
    DynamicVar(Box<Expr>),
    ConstFetch(Name),
    Array(Vec<ArrayItem>),
    List(Vec<ArrayItem>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Cast {
        kind: CastKind,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        /// `None` for Elvis operator `$a ?: $b`.
        then_expr: Option<Box<Expr>>,
        else_expr: Box<Expr>,
    },
    Index {
        target: Box<Expr>,
        /// `None` for append `$a[]`.
        index: Option<Box<Expr>>,
    },
    Prop {
        target: Box<Expr>,
        prop: PropName,
        nullsafe: bool,
    },
    StaticProp {
        class: ClassRef,
        prop: PropName,
    },
    ClassConst {
        class: ClassRef,
        name: PropName,
    },
    Call {
        callee: Callee,
        args: CallArgs,
    },
    MethodCall {
        target: Box<Expr>,
        method: PropName,
        nullsafe: bool,
        args: CallArgs,
    },
    StaticCall {
        class: ClassRef,
        method: PropName,
        args: CallArgs,
    },
    New {
        class: ClassRef,
        args: Vec<Arg>,
    },
    AnonClass {
        is_readonly: bool,
        extends: Option<Name>,
        implements: Vec<Name>,
        args: Vec<Arg>,
        members: Vec<ClassMember>,
    },
    Clone(Box<Expr>),
    InstanceOf {
        expr: Box<Expr>,
        class: ClassRef,
    },
    Closure {
        attributes: Vec<Attribute>,
        is_static: bool,
        by_ref: bool,
        params: Vec<Param>,
        uses: Vec<ClosureUse>,
        return_type: Option<TypeHint>,
        body: Vec<Stmt>,
    },
    ArrowFn {
        attributes: Vec<Attribute>,
        is_static: bool,
        by_ref: bool,
        params: Vec<Param>,
        return_type: Option<TypeHint>,
        body: Box<Expr>,
    },
    Match {
        cond: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Throw(Box<Expr>),
    Yield {
        key: Option<Box<Expr>>,
        value: Option<Box<Expr>>,
    },
    YieldFrom(Box<Expr>),
    Isset(Vec<Expr>),
    Empty(Box<Expr>),
    Print(Box<Expr>),
    Include {
        kind: IncludeKind,
        expr: Box<Expr>,
    },
    Exit(Option<Box<Expr>>),
    Eval(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum PropName {
    Ident(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum ClassRef {
    Name(Name),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum Callee {
    Name(Name),
    Expr(Box<Expr>),
}
