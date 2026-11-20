//! Cached against uncached (C18, host/src/runtime_cache.rs): a program compiled with the
//! runtime's embedded native code must behave as one compiled from source alone. Every corpus
//! program whose `main` takes no capabilities, and a kiogen batch, run under four engines: the
//! cache or a plain engine, each compiling on one thread or in parallel. Value, failed flag,
//! error message and span, host error text and fuel must be identical.
//!
//! `KIO_CACHE_DIFF_GEN` generated programs (default 25) from `KIO_GEN_SEED` (as differential_gen);
//! `KIO_CACHE_DIFF_EVERY` takes every nth corpus program (default 5; 1 is all, about 6 minutes).

use kio_host::{Kio, Limits};
use kiogen::Features;
use std::path::Path;

#[path = "../../lang/tests/common/corpus.rs"]
mod corpus;

/// What a run gave: the value and failed flag, or the error's message and span; and its fuel.
type Run = Result<(Result<(String, bool), (String, u32, u32)>, Option<u64>), String>;

fn run(kio: &mut Kio, src: &str) -> Run {
    match kio.run(src, vec![]) {
        Ok(Ok(o)) => Ok((Ok((o.value, o.failed)), kio.used().map(|u| u.fuel))),
        Ok(Err(d)) => Ok((
            Err((d[0].message.clone(), d[0].start, d[0].end)),
            kio.used().map(|u| u.fuel),
        )),
        Err(e) => Err(format!("{e:#}")),
    }
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |v| v.parse().expect(name))
}

fn limits() -> Limits {
    Limits {
        fuel: 2_000_000_000,
        ..Limits::default()
    }
}

/// `(cached, parallel)`: the engine `kio run` uses first, then the three others.
const ENGINES: [(bool, bool); 4] = [(true, true), (true, false), (false, true), (false, false)];

/// Runs every program on the four engines; a disagreement with the plain parallel engine fails,
/// naming the program. Returns how many programs agreed.
fn agree(programs: impl Iterator<Item = String>) -> usize {
    let mut runners: Vec<Kio> = ENGINES
        .iter()
        .map(|&(cached, parallel)| Kio::new_with_engine(limits(), cached, parallel).unwrap())
        .collect();
    let mut agreed = 0;
    let mut mismatches = Vec::new();
    for src in programs {
        let runs: Vec<Run> = runners.iter_mut().map(|k| run(k, &src)).collect();
        let plain = &runs[2];
        if runs.iter().all(|r| r == plain) {
            agreed += 1;
        } else {
            let shown: Vec<String> = ENGINES
                .iter()
                .zip(&runs)
                .map(|((c, p), r)| format!("cached {c} parallel {p}: {r:?}"))
                .collect();
            mismatches.push(format!("{}\n{src}", shown.join("\n")));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} programs differ between engines:\n{}",
        mismatches.len(),
        mismatches.join("\n----\n")
    );
    agreed
}

#[test]
fn corpus_programs_agree_cached_and_uncached() {
    let caps = lang::CapTable::v0();
    let every = env("KIO_CACHE_DIFF_EVERY", 5) as usize;
    let mut seen = std::collections::HashSet::new();
    let mut programs = Vec::new();
    for files in corpus::sources(Path::new("../")) {
        let [(_, src)] = files.as_slice() else { continue };
        if !seen.insert(src.clone()) {
            continue;
        }
        let Ok(program) = lang::check(src, &caps) else { continue };
        if program.main_params().is_empty() {
            programs.push(src.clone());
        }
    }
    let agreed = agree(programs.into_iter().step_by(every));
    println!("corpus programs agreed on all four engines: {agreed}");
    assert!(agreed >= 100 / every, "only {agreed} programs");
    let (hits, misses) = Kio::runtime_cache_stats();
    assert!(
        hits > misses,
        "the cached engines found the runtime: {hits} hits, {misses} misses"
    );
}

#[test]
fn generated_programs_agree_cached_and_uncached() {
    let n = env("KIO_CACHE_DIFF_GEN", 25);
    let first = env("KIO_GEN_SEED", 0x6b69_6f00);
    let features = Features::default();
    let agreed = agree((first..first + n).map(|seed| kiogen::program(seed, &features)));
    assert_eq!(agreed as u64, n);
}
