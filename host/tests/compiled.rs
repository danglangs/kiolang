use kio_host::{Compiled, Kio, Limits};

#[test]
fn forty_plus_two_compiled() {
    let out = Compiled::run("fn main() -> Int { 40 + 2 }", Limits::default())
        .unwrap()
        .unwrap();
    assert_eq!((out.value.as_str(), out.failed), ("42", false));
}

#[test]
fn overflow_matches_the_interpreter() {
    let src = "fn main() -> Int { 9223372036854775807 + 1 }";
    let compiled = &Compiled::run(src, Limits::default()).unwrap().unwrap_err()[0];
    let interp = &Kio::new(Limits::default())
        .unwrap()
        .run(src, vec![])
        .unwrap()
        .unwrap_err()[0];
    assert_eq!(compiled.msg, interp.message);
    assert_eq!(
        (compiled.span.start, compiled.span.end),
        (interp.start as usize, interp.end as usize)
    );
}
