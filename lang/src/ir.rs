//! The lowered program: what the checker proved, with every name resolved.
//!
//! Lowering (P3b) builds a `Module` from a checked `Program`; the interpreter (P3c) and the Wasm
//! code generator run it. Invariants, which lowering establishes and nothing re-checks:
//! - Every id is in range: a `FuncId` indexes `Module::funcs`, a `LocalId` the function's
//!   `locals`, a field or variant index its declaration.
//! - Every `Expr::ty` is the type the checker gave that expression.
//! - Nothing is looked up by name at run time. Names remain only for display and errors.
//!
//! `Ty` is its own type, not the checker's, so that structs and enums are ids here.

use crate::Span;
use std::rc::Rc;

macro_rules! id {
    ($($name:ident),*) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name(pub u32);
    )*};
}
id!(FuncId, StructId, EnumId, LocalId, CapId);

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub funcs: Vec<Func>,
    pub structs: Vec<StructDef>,
    /// The capability error enums such as `IoErr` first, in `CapTable` order, so their ids are
    /// fixed for the host; then the program's enums.
    pub enums: Vec<EnumDef>,
    /// Capability types, in `CapTable` order; a method is its index in `CapDef::methods`.
    pub caps: Vec<CapDef>,
    pub main: FuncId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    /// The checker's key, such as `f` or `web::f`.
    pub name: String,
    /// Every local slot; parameters come first, in order.
    pub locals: Vec<Local>,
    pub params: u32,
    pub ret: Ty,
    pub body: Block,
    pub span: Span,
}

/// A slot's name and declaration, for errors such as `main`'s "no grant for `out: Dir`", and
/// for reading lowered code.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    /// The checker's key, such as `web::Point`.
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    /// The checker's key, such as `web::Status`.
    pub name: String,
    pub variants: Vec<(String, Vec<Ty>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapDef {
    pub name: String,
    /// Name, parameter types and return type of each method.
    pub methods: Vec<(String, Vec<Ty>, Ty)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Bytes,
    Unit,
    Struct(StructId),
    Enum(EnumId),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Cap(CapId),
    /// A function value: parameter types and return type. Never mentions a capability.
    Fn(Vec<Ty>, Box<Ty>),
    /// An expression that never produces a value, such as `return`. Only ever an expression's
    /// own type: lowering never puts it inside another type.
    Never,
}

/// Which enum a variant belongs to; also the tag the interpreter keeps on an enum value.
/// `Option` and `Result` are built in, with the variant indexes `None` 0, `Some` 1, `Ok` 0,
/// `Err` 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumRef {
    Option,
    Result,
    User(EnumId),
}

/// A block without a tail is `()`, or diverges where its context needs a value (the enclosing
/// `Expr::ty`, or `Func::ret`).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let(LocalId, Expr),
    /// `=` when the operator is `None`; `+=` is `Add`, `FAdd` or `Concat`, `-=` is `Sub` or `FSub`. An overflow
    /// reports the statement's span.
    Assign(Place, Option<BinOp>, Expr),
    Return(Option<Expr>),
    While(Expr, Block),
    Loop(Block),
    /// `for x in xs`: iterates over `xs` as it was when the loop started.
    ForList(LocalId, Expr, Block),
    /// `for i in a..b`
    ForRange(LocalId, Expr, Expr, Block),
    Break,
    Continue,
    Expr(Expr),
}

/// A local, or a part of one reached through fields and indexes.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub root: LocalId,
    /// From the root outward. Indexes are evaluated in this order, before the assigned value
    /// or the method's arguments.
    pub steps: Vec<PlaceStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaceStep {
    Field(u32),
    /// An index into a `List`, and the span of the whole `xs[i]`, which a bounds error reports.
    Index(Expr, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Unit,
    Local(LocalId),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    Block(Block),
    Call(FuncId, Vec<Expr>),
    /// A function named as a value.
    FnRef(FuncId),
    /// A call through a value of `fn` type: the callee is evaluated first, then the arguments.
    CallValue(Box<Expr>, Vec<Expr>),
    /// A capability method: the capability is always a local, since capabilities are
    /// parameters only. `method` indexes `CapDef::methods`.
    CapCall(LocalId, u32, Vec<Expr>),
    /// A built-in method that does not change its receiver.
    Builtin(Builtin, Box<Expr>, Vec<Expr>),
    /// A built-in method that changes a local in place: `push`, `insert`, `remove`.
    Mutate(Place, Mut, Vec<Expr>),
    List(Vec<Expr>),
    MapNew,
    /// Each field's declaration index and value, in source order, which is evaluation order.
    StructLit(StructId, Vec<(u32, Expr)>),
    Field(Box<Expr>, u32),
    Index(Box<Expr>, Box<Expr>),
    Variant(EnumRef, u32, Vec<Expr>),
    /// Also `if let`, as its pattern's arm and a `_` arm.
    Match(Box<Expr>, Vec<Arm>),
    /// `e?` on an `Option` or `Result`.
    Try(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `-` on a `Float`.
    FNeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    /// Structural equality on any type without capabilities.
    Eq,
    Ne,
    And,
    Or,
    /// `+` on two `Str`.
    Concat,
    /// Arithmetic and order on two `Float`, as Rust's `f64`; `==` on floats is `Eq`.
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
    FLt,
    FLe,
    FGt,
    FGe,
}

/// Every built-in method that leaves its receiver unchanged, so evaluation never matches on a
/// method name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    IntToStr,
    StrLen,
    StrToBytes,
    StrSplit,
    StrLines,
    StrTrim,
    StrContains,
    StrStartsWith,
    StrParseInt,
    StrEndsWith,
    StrStripPrefix,
    StrStripSuffix,
    StrFind,
    StrReplace,
    ListContains,
    BytesLen,
    BytesToStr,
    ListLen,
    MapLen,
    MapKeys,
    MapGet,
    OptionUnwrapOr,
    OptionIsSome,
    OptionIsNone,
    ResultUnwrapOr,
    ResultIsOk,
    ResultIsErr,
    ResultOk,
    ResultErr,
    OptionOkOr,
    IntToFloat,
    StrParseFloat,
    FloatToStr,
    FloatToInt,
    FloatAbs,
    FloatFloor,
    FloatCeil,
    FloatRound,
    FloatSqrt,
    FloatIsNan,
}

/// Every built-in method that changes its receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mut {
    ListPush,
    ListSort,
    ListPop,
    BytesPush,
    MapInsert,
    MapRemove,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pat {
    Wild,
    Bind(LocalId),
    Int(i64),
    Str(String),
    Bool(bool),
    /// A variant of the matched value's enum, with a pattern for each payload field.
    Variant(u32, Vec<Pat>),
}
