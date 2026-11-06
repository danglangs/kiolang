pub mod ast;
mod caps_gen;
pub mod check;
pub mod eval;
pub mod ir;
pub mod lex;
pub mod parse;
pub mod report;

pub use check::{check, check_files, CapTable, Program};
pub use eval::{run, Caps, Key, RunErr, Value};
pub use report::report;

/// Byte offsets `start..end` into source file number `file`; 0 is the root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub file: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
    pub hint: Option<String>,
}
