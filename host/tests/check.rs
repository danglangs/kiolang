use host::Kio;
use lang::{check, CapTable};

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
