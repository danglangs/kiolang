use crate::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Fn(Fn),
    Struct(Struct),
    Enum(Enum),
    /// `mod name;`
    Mod(String, Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fn {
    pub public: bool,
    pub name: String,
    pub params: Vec<Param>,
    /// `()` when the source has no `->`.
    pub ret: Type,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Struct {
    pub public: bool,
    pub name: String,
    /// Declared fields, which have the shape of parameters.
    pub fields: Vec<Param>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    pub public: bool,
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    /// Payload types; empty for `A`.
    pub fields: Vec<Type>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Unit,
    /// `List<Int>` is `Named("List", [Int])`.
    Named(String, Vec<Type>),
    /// `web::Point`: the type after `web::`.
    Module(String, Box<Type>),
    /// `fn(Int, Str) -> Bool`; the return type is `()` when the source has no `->`.
    Fn(Vec<Type>, Box<Type>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The final expression without `;`, which is the block's value.
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtKind {
    Let {
        name: String,
        mutable: bool,
        ty: Option<Type>,
        init: Expr,
    },
    /// `op` is `=`, `+=` or `-=`; `target` is a name, field or index.
    Assign {
        target: Expr,
        op: &'static str,
        value: Expr,
    },
    Return(Option<Expr>),
    While {
        cond: Expr,
        body: Block,
    },
    Loop {
        body: Block,
    },
    For {
        name: String,
        iter: Iter,
        body: Block,
    },
    Break,
    Continue,
    Expr(Expr),
}

/// What a `for` loop runs over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Iter {
    /// `for x in xs`
    List(Expr),
    /// `for i in a..b`
    Range(Expr, Expr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Int(i64),
    Str(String),
    Bool(bool),
    Unit,
    Name(String),
    /// `op` is the operator text: `-` or `!`.
    Unary(&'static str, Box<Expr>),
    /// `op` is the operator text, such as `+` or `&&`.
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// The else branch is a `Block`, another `If` or an `IfLet`.
    If(Box<Expr>, Block, Option<Box<Expr>>),
    /// `if let pat = e { .. } else ..`, with an else branch as for `If`.
    IfLet(Box<Pat>, Box<Expr>, Block, Option<Box<Expr>>),
    Block(Block),
    Call(Box<Expr>, Vec<Expr>),
    /// `[a, b]`
    List(Vec<Expr>),
    Method(Box<Expr>, String, Vec<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `Name { f: e, .. }`, with fields in source order.
    StructLit(String, Vec<(String, Expr)>),
    /// `Enum::Variant`; a payload is a `Call` on it.
    Path(String, String),
    /// `web::f`, `web::Point { .. }` or `web::Status::Paid`: the expression after `web::`.
    Module(String, Box<Expr>),
    Match(Box<Expr>, Vec<Arm>),
    /// `e?`
    Try(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatKind {
    /// `_`
    Wild,
    /// Binds the value; the checker decides whether it is a variant such as `None`.
    Name(String),
    Int(i64),
    Str(String),
    Bool(bool),
    /// `Enum::Variant(a, _)` or `Variant(a)`, with a pattern for each payload field.
    Variant(Option<String>, String, Vec<Pat>),
    /// `web::Status::Paid(x)`: the pattern after `web::`.
    Module(String, Box<Pat>),
}
