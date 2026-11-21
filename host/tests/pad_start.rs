//! L31b: `pad_start`'s width is bounded the same way compiled and interpreted (adversarial
//! review: a width of 2^32 or more returned the text unchanged compiled while the interpreter
//! allocated it, and 2^31 to 2^32 trapped at no error site).

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn compiled(src: &str) -> Run {
    match Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

#[test]
fn widths_outside_the_range_fail_both_ways() {
    for w in ["-1", "2147483648", "3000000000", "4294967300", "9223372036854775807"] {
        let src = format!("fn main() -> Str {{ \"ab\".pad_start({w}, \"x\") }}");
        let c = compiled(&src);
        assert_eq!(c, interpreted(&src), "{w}");
        let (msg, start, end) = c.expect_err(w);
        assert_eq!(
            msg,
            format!("Str.pad_start({w}, ..): the width must be 0 to 2147483647")
        );
        assert_eq!(&src[start..end], format!("\"ab\".pad_start({w}, \"x\")"));
    }
}

#[test]
fn widths_in_range_agree() {
    for w in ["0", "2", "5"] {
        let src = format!("fn main() -> Str {{ \"ab\".pad_start({w}, \"xy\") }}");
        assert_eq!(compiled(&src), interpreted(&src), "{w}");
    }
}
