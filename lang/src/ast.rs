use crate::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Fn(Fn),
    Struct(Struct),
    Enum(Enum),
    /// `mod name;`
    Mod(String, Span),
    Const(Const),
}

/// `const NAME: T = lit;`: `T` is `Int`, `Float`, `Bool` or `Str`; the checker requires `value`
/// to be a literal of it (a number may have a leading `-`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Const {
    pub public: bool,
    pub name: String,
    pub ty: Type,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fn {
    pub public: bool,
    pub name: String,
    /// Type parameters `<T, U: Ord>`; empty for a function that is not generic.
    pub tparams: Vec<TParam>,
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
    /// Type parameters `<T, U: Ord>`, as on `Fn`.
    pub tparams: Vec<TParam>,
    /// Declared fields, which have the shape of parameters.
    pub fields: Vec<Param>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    pub public: bool,
    pub name: String,
    /// Type parameters `<T, U: Ord>`, as on `Fn`.
    pub tparams: Vec<TParam>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

/// A type parameter `T`, `T: Eq` or `T: Ord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TParam {
    pub name: String,
    /// The strongest bound written: `T: Eq + Ord` is `Ord`, which implies `Eq`.
    pub bound: Option<TBound>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TBound {
    Eq,
    Ord,
}

impl std::fmt::Display for TBound {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            TBound::Eq => "Eq",
            TBound::Ord => "Ord",
        })
    }
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
    /// `let P: T = e else { .. };`; the else block must not finish normally.
    LetElse {
        pat: Pat,
        ty: Option<Type>,
        init: Expr,
        els: Block,
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
    /// The bits of an `f64`, so that the AST stays `Eq`.
    Float(u64),
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
    /// `Name { f: e, .. }`, with fields in source order, and the base of `..base`, if any.
    StructLit(String, Vec<(String, Expr)>, Option<Box<Expr>>),
    /// `Enum::Variant`; a payload is a `Call` on it.
    Path(String, String),
    /// `web::f`, `web::Point { .. }` or `web::Status::Paid`: the expression after `web::`.
    Module(String, Box<Expr>),
    Match(Box<Expr>, Vec<Arm>),
    /// `e?`
    Try(Box<Expr>),
    /// `first::<Int>`: a function named with explicit type arguments.
    Turbofish(String, Vec<Type>),
    /// `|x: Int, y| body`; `|| body` has no parameters.
    Closure(Vec<ClosureParam>, Box<Expr>),
}

/// A closure parameter; without a type, the checker takes it from the expected `fn` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<Type>,
    pub span: Span,
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
    /// `P1 | P2 | ..`, tried left to right; each alternative binds the same names.
    Or(Vec<Pat>),
}
