use host::Kio;
use lang::{check, CapTable};
use std::collections::BTreeMap;

#[test]
fn summarize_reports_two_lines() {
    let src = std::fs::read_to_string("../examples/summarize.kio").unwrap();
    let mut kio = Kio::new().unwrap();
    let report = kio.check(&src).unwrap().unwrap();
    let want = lang::report(&check(&src, &CapTable::v0()).unwrap());
    assert_eq!(report.lines.len(), 2);
    assert_eq!(report.lines.join("\n"), want);
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn unknown_name_matches_lang_diag() {
    let src = "fn main() -> Int {\n    nope\n}\n";
    let mut kio = Kio::new().unwrap();
    let got = kio.check(src).unwrap().unwrap_err();
    let want = check(src, &CapTable::v0()).unwrap_err();
    assert_eq!(got.len(), want.len());
    let (g, w) = (&got[0], &want[0]);
    assert_eq!((g.start as usize, g.end as usize), (w.span.start, w.span.end));
    assert_eq!(g.message, w.msg);
    assert_eq!(g.hint, w.hint);
    assert!(g.hint.is_some());
    assert_eq!(kio.import_calls(), 0);
}

/// No clock, randomness or environment reaches the guest, so fresh instances agree exactly.
#[test]
fn check_is_deterministic_across_instances() {
    let src = std::fs::read_to_string("../examples/invoices.kio").unwrap();
    let run = || Kio::new().unwrap().check(&src).unwrap().unwrap().lines;
    assert_eq!(run(), run());
}

type Dirs = Vec<(String, BTreeMap<String, Vec<u8>>)>;

fn dir(name: &str, files: &[(&str, &[u8])]) -> (String, BTreeMap<String, Vec<u8>>) {
    let files = files.iter().map(|(p, b)| (p.to_string(), b.to_vec()));
    (name.to_string(), files.collect())
}

fn invoices() -> String {
    std::fs::read_to_string("../examples/invoices.kio").unwrap()
}

fn grants() -> Dirs {
    vec![dir("input", &[("invoices.csv", b"a,40\nb,2\nbad\n")]), dir("out", &[])]
}

#[test]
fn invoices_writes_total() {
    let mut kio = Kio::new().unwrap();
    let (report, value) = kio.run(&invoices(), grants()).unwrap().unwrap();
    assert_eq!(report.lines.len(), 4);
    assert_eq!(value, r#"Enum("Result", 0, [Unit])"#);
    assert_eq!(kio.files("out").unwrap()["total.txt"], b"42");
    assert_eq!(kio.import_calls(), 2);
}

#[test]
fn missing_file_is_not_found() {
    let src = "fn main(d: Dir) -> Result<Bytes, IoErr> { d.read(\"x\") }";
    let mut kio = Kio::new().unwrap();
    let (_, value) = kio.run(src, vec![dir("d", &[])]).unwrap().unwrap();
    assert_eq!(value, r#"Enum("Result", 1, [Enum("IoErr", 0, [])])"#);
    assert_eq!(kio.import_calls(), 1);
}

fn rejected(grants: Dirs, message: &str, hint: &str) {
    let mut kio = Kio::new().unwrap();
    let diags = kio.run(&invoices(), grants).unwrap().unwrap_err();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, message);
    assert!(diags[0].hint.as_deref().unwrap().contains(hint));
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn duplicate_grant() {
    let mut g = grants();
    g.push(dir("out", &[]));
    rejected(g, "grant `out` is given more than once", "one name");
}

#[test]
fn extra_grant() {
    let mut g = grants();
    g.push(dir("extra", &[]));
    rejected(g, "grant `extra` is not a parameter of `main`", "input, out");
}

#[test]
fn missing_grant_is_no_grant_error() {
    let mut kio = Kio::new().unwrap();
    let diags = kio.run(&invoices(), vec![dir("input", &[])]).unwrap().unwrap_err();
    assert_eq!(diags[0].message, "no grant for `out: Dir`");
    assert_eq!(kio.import_calls(), 0);
}

#[test]
fn runtime_error_has_span() {
    let src = "fn main(d: Dir) -> Int {\n    let xs: List<Int> = [];\n    xs[1]\n}\n";
    let mut kio = Kio::new().unwrap();
    let diags = kio.run(src, vec![dir("d", &[])]).unwrap().unwrap_err();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].end > diags[0].start);
    assert!(diags[0].hint.is_none());
}
