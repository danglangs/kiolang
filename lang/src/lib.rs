pub mod ast;
pub mod check;
pub mod eval;
pub mod lex;
pub mod parse;

pub use check::{check, CapTable, Program};
pub use eval::{run, Caps, RunErr, Value};

/// Byte offsets into the source, `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
    pub hint: Option<String>,
}
