extern crate alloc;

pub mod ast;
mod caps_gen;
pub mod check;
pub mod digest;
pub mod eval;
pub mod hmap;
pub mod ir;
pub mod json;
pub mod lex;
pub mod lower;
pub mod parse;
pub mod prop;
pub mod report;
pub mod sort;

pub use check::{check, check_files, check_files_for_tests, CapTable, Program, TestFn};
pub use eval::{run, run_entry, Caps, Key, RunErr, Value};
pub use report::{
    authority, entry_authority, entry_report, main_http_verbs, reachable, report, report_diff, shown, shown_report,
    Authority, AuthorityDiff, Shown, COVERS, HTTP_METHODS,
};

/// Byte offsets `start..end` into source file number `file`; 0 is the root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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
