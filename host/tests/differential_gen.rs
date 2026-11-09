//! Generated programs (tools/kiogen) give the same result compiled as interpreted.
//! `KIO_GEN_N` programs (default 300) from seed `KIO_GEN_SEED` (default fixed) on; a mismatch
//! prints the seed and the source, so `KIO_GEN_SEED=<seed> KIO_GEN_N=1` reruns it alone.

use kio_host::{Compiled, Kio, Limits, MAX_STACK};
use kiogen::Features;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// The displayed value and failed flag, or the first error as (message, span).
type Run = Result<(String, bool), (String, usize, usize)>;

/// As in differential.rs: limit stops are not compared (decision 6), so they are `None`.
fn finish(r: wasmtime::Result<Run>) -> Option<Run> {
    let is_limit = |m: &str| {
        ["out of fuel", "stack overflow", "evaluation is too deep"]
            .iter()
            .any(|l| m.contains(l))
    };
    match r {
        Ok(Err((m, _, _))) if is_limit(&m) => None,
        Err(e) if is_limit(&format!("{e:#}")) => None,
        Err(e) => Some(Err((format!("host error: {e:#}"), 0, 0))),
        Ok(r) => Some(r),
    }
}

fn limits() -> Limits {
    Limits {
        fuel: 2_000_000_000,
        ..Limits::default()
    }
}

fn compiled(src: &str) -> Option<Run> {
    finish(Compiled::run(src, limits()).map(|r| {
        r.map(|o| (o.value, o.failed))
            .map_err(|d| (d[0].msg.clone(), d[0].span.start, d[0].span.end))
    }))
}

fn interpreted(src: &str) -> Option<Run> {
    finish(Kio::new(limits()).unwrap().run(src, vec![]).map(|r| {
        r.map(|o| (o.value, o.failed))
            .map_err(|d| (d[0].message.clone(), d[0].start as usize, d[0].end as usize))
    }))
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |v| v.parse().expect(name))
}

fn generated() {
    let n = env("KIO_GEN_N", 300);
    let first = env("KIO_GEN_SEED", 0x6b69_6f00);
    let caps = lang::CapTable::v0();
    let features = Features::default();
    let (mut values, mut errors, mut limited) = (0, 0, 0);
    for seed in first..first + n {
        let src = kiogen::program(seed, &features);
        let fail = |what: String| -> ! { panic!("seed {seed}: {what}\n{src}") };
        if let Err(d) = lang::check(&src, &caps) {
            let line = src[..d[0].span.start].lines().count();
            let at = &src[d[0].span.start..d[0].span.end];
            fail(format!(
                "the checker rejects it (a generator bug): {} at line {line}, `{at}`",
                d[0].msg
            ));
        }
        let c = match catch_unwind(AssertUnwindSafe(|| compiled(&src))) {
            Ok(c) => c,
            Err(p) => fail(format!("codegen panicked: {:?}", p.downcast_ref::<String>())),
        };
        match (c, interpreted(&src)) {
            (Some(c), Some(i)) if c != i => fail(format!("compiled: {c:?}\ninterpreted: {i:?}")),
            (Some(Ok(_)), Some(_)) => values += 1,
            (Some(Err(_)), Some(_)) => errors += 1,
            _ => limited += 1,
        }
    }
    println!("{n} programs from seed {first}: {values} values, {errors} runtime errors, {limited} stopped by a limit");
    assert!(values >= n / 3, "too few programs ran to a value");
}

#[test]
fn generated_programs_agree() {
    // Wasm runs on this thread's stack, so it needs room above `--stack`, as `kio` gives it.
    let t = std::thread::Builder::new().stack_size(4 * MAX_STACK);
    t.spawn(generated).unwrap().join().unwrap();
}

#[test]
fn same_seed_same_source() {
    let f = Features::default();
    for seed in 0..20 {
        assert_eq!(kiogen::program(seed, &f), kiogen::program(seed, &f));
    }
    assert_ne!(kiogen::program(1, &f), kiogen::program(2, &f));
}
