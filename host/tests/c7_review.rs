//! C7: regression tests from the adversarial review of the compiled path. Each test is a finding:
//! what a program, a crafted IR or a limit did, and what the host must do instead.

use kio_host::{Interp, Kio, Limits, RunResult};
use lang::ir::{ExprKind, LocalId, Module};
use std::sync::{Arc, Mutex};

type Run = Result<(String, bool), (String, usize, usize)>;

fn compiled(src: &str) -> Run {
    match Kio::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(o) => Ok((o.value, o.failed)),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(Limits::default()).unwrap().run(src, vec![]).unwrap() {
        Ok(o) => Ok((o.value, o.failed)),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

fn host_error(r: wasmtime::Result<RunResult>) -> String {
    format!("{:#}", r.unwrap_err())
}

/// `==` through a container, struct, enum or `Map` compared `Int` slots after turning them into
/// 32-bit pointers, so values that differ only above bit 31 were equal: `[4294967296] == [0]`.
#[test]
fn int_equality_in_containers_keeps_the_high_bits() {
    let cases = [
        ("fn main() -> Bool { [4294967296] == [0] }", "false"),
        ("fn main() -> Bool { Some(4294967296) == Some(0) }", "false"),
        ("fn main() -> Bool { [4294967296].contains(0) }", "false"),
        ("fn main() -> Bool { [-1] == [4294967295] }", "false"),
        ("struct P { a: Int } fn main() -> Bool { P { a: 4294967297 } == P { a: 1 } }", "false"),
        ("enum K { A(Int) } fn main() -> Bool { K::A(8589934592) == K::A(0) }", "false"),
        (
            "fn main() -> Bool { let mut m: Map<Int, Int> = Map::new(); m.insert(1, 4294967296); let mut n: Map<Int, Int> = Map::new(); n.insert(1, 0); m == n }",
            "false",
        ),
        (
            "fn main() -> Bool { let r: Result<Int, Str> = Ok(4294967296); let s: Result<Int, Str> = Ok(4294967296); r == s }",
            "true",
        ),
        ("fn main() -> Bool { [true, false] == [true, false] }", "true"),
    ];
    for (src, want) in cases {
        assert_eq!(both(src), Ok((want.to_string(), false)), "{src}");
    }
}

/// A runtime panic traps at the runtime's internal-error site, outside the program's site table:
/// the host reports an unknown site, never a program error or an index panic. The crafted IR
/// names a variant the enum does not have, with and without a payload (the second is a pinned
/// static the runtime cannot display).
#[test]
fn an_internal_error_site_is_not_a_program_error() {
    for src in [
        "fn main() -> Option<Int> { Some(1) }",
        "fn main() -> Option<Int> { None }",
    ] {
        let mut c = Kio::new(Limits::default()).unwrap();
        let r = c.run_edited(src, vec![], vec![], |m: &mut Module| {
            let main = &mut m.funcs[m.main.0 as usize];
            let ExprKind::Variant(_, v, _) = &mut main.body.tail.as_mut().unwrap().kind else {
                panic!("a variant")
            };
            *v = 99;
        });
        let e = host_error(r);
        assert!(
            e.starts_with("compiled program trapped at an unknown error site"),
            "{src}: {e}"
        );
        assert!(e.ends_with(&format!("kio runtime error at site {}", u32::MAX)), "{e}");
    }
}

/// A crafted IR standing in for a codegen type confusion: `Dir.read` given a `Bool`, the `Dir`
/// handle, or a `List<Int>` where it expects a `Str` pointer. The runtime reads a bogus object;
/// whatever it sends, the host neither panics nor leaves the grant, and reports cleanly.
#[test]
fn a_bogus_pointer_argument_does_not_crash_the_host() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.txt"), "secret").unwrap();
    let src = r#"fn main(d: Dir) -> Result<Bytes, IoErr> { let p = "a.txt"; d.read(p) }"#;
    fn path_arg(m: &mut Module) -> &mut lang::ir::Expr {
        let main = &mut m.funcs[m.main.0 as usize];
        match &mut main.body.tail.as_mut().unwrap().kind {
            ExprKind::CapCall(_, _, args) => &mut args[0],
            k => panic!("{k:?}"),
        }
    }
    let edits: [fn(&mut Module); 3] = [
        |m| path_arg(m).kind = ExprKind::Bool(true),
        |m| path_arg(m).kind = ExprKind::Local(LocalId(0)),
        |m| {
            let arg = path_arg(m);
            let n = lang::ir::Expr {
                kind: ExprKind::Int(0x4141_4141_4141),
                ty: lang::ir::Ty::Int,
                span: arg.span,
            };
            arg.kind = ExprKind::List(vec![n.clone(), n]);
            arg.ty = lang::ir::Ty::List(Box::new(lang::ir::Ty::Int));
        },
    ];
    for edit in edits {
        let d = kio_host::open_dir(t.path()).unwrap();
        let mut c = Kio::new(Limits::default()).unwrap();
        match c.run_edited(src, vec![("d".into(), d)], vec![], edit) {
            // The bogus object read as a path: an empty or garbage string, refused.
            Ok(Ok(o)) => assert_eq!((o.value.as_str(), o.failed), ("IoErr::InvalidPath", true)),
            Ok(Err(d)) => panic!("a program error: {}", d[0].message),
            // The runtime noticed (a handle is not an object): its internal-error site.
            Err(e) => {
                let e = format!("{e:#}");
                assert!(
                    e.starts_with("compiled program trapped at an unknown error site"),
                    "{e}"
                );
            }
        }
        assert!(c.import_calls() <= 1);
    }
}

/// Out of memory while the host lowers a capability result into the runtime's `cabi_realloc`
/// (the `IN_PLACE` path): the run ends as "out of memory", not an internal error, and the
/// audit log still ends.
#[test]
fn out_of_memory_in_a_host_result_is_explained() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("big.bin"), vec![7u8; 3 << 20]).unwrap();
    let d = kio_host::open_dir(t.path()).unwrap();
    let limits = Limits {
        memory: 2 << 20,
        ..Limits::default()
    };
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Log::default();
    let mut c = Kio::new(limits).unwrap();
    c.audit(log.clone(), vec![("d".into(), "/d".into())]);
    let src = r#"fn main(d: Dir) -> Int { match d.read("big.bin") { Ok(b) => b.len(), Err(_) => -1 } }"#;
    let e = host_error(c.run_with(src, vec![("d".into(), d)], vec![]));
    assert!(
        e.starts_with("out of memory (limit 2 MB; raise it with --memory)"),
        "{e}"
    );
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains(r#""method":"read","arg":"big.bin","bytes":3145728,"result":"ok""#),
        "{text}"
    );
    assert!(text.contains(r#"{"event":"end","outcome":"error","calls":1"#), "{text}");
}

/// Wasm validators cap a function at 1,000 parameters and 50,000 locals; codegen panicked or
/// produced an invalid module past either (the interpreter ran it: a C7 finding). The checker now
/// refuses more than 999 parameters, on both paths; hidden locals are reused (C7a), so 17,000
/// calls of three arguments in one function (51,000 hidden locals before) compile.
#[test]
fn a_function_past_the_wasm_limits_is_an_error_not_a_panic() {
    let n = 1001;
    let params: Vec<String> = (0..n).map(|i| format!("p{i}: Int")).collect();
    let args: Vec<String> = (0..n).map(|i| format!("{i}")).collect();
    let src = format!(
        "fn g({}) -> Int {{ p0 + p{} }}\nfn main() -> Int {{ g({}) }}",
        params.join(", "),
        n - 1,
        args.join(", ")
    );
    let (msg, ..) = both(&src).unwrap_err();
    assert_eq!(msg, "`g` takes 1001 parameters; at most 999");

    let calls: String = (0..17_000).map(|i| format!("    s += g({i}, 2, 3);\n")).collect();
    let src = format!("fn g(a: Int, b: Int, c: Int) -> Int {{ a + b - c }}\nfn main() -> Int {{\n    let mut s = 0;\n{calls}    s\n}}");
    assert_eq!(both(&src), Ok(("144474500".into(), false)));
}

/// A `!`-typed index expression in a place write (`xs[{ return 7; }][0] = v`): the interpreter
/// returns 7; codegen panicked (`an Int index`, `FuncGen::step_slot`) because a `!` operand has
/// no hidden local. Ignored until that is fixed.
#[test]
fn a_never_typed_index_in_a_place_write() {
    let cases = [
        r#"fn f() -> Int { let mut xs = [["a"]]; xs[{ return 7; }][0] = "z"; 0 } fn main() -> Int { f() }"#,
        r#"fn f() -> Int { let mut xs = [["a"]]; xs[0][{ return 7; }] = "z"; 0 } fn main() -> Int { f() }"#,
        r#"fn f() -> Int { let mut xs = [["a"]]; xs[{ return 7; }].push("z"); 0 } fn main() -> Int { f() }"#,
        r#"fn f() -> Int { let mut xs = [[1]]; xs[{ return 7; }][0] += 1; 0 } fn main() -> Int { f() }"#,
    ];
    for src in cases {
        assert_eq!(both(src), Ok(("7".into(), false)), "{src}");
    }
}

/// `--memory` below the module's initial memory (18 pages: the runtime's data and stack) was
/// Wasmtime's "memory minimum size of 18 pages exceeds memory limits", and a server's was the
/// pool's "module memory does not fit in pooling allocator requirements"; now both are out of
/// memory, naming what the program starts with (C7a).
#[test]
fn memory_below_the_initial_memory_is_out_of_memory() {
    let limits = Limits {
        memory: 1 << 20,
        ..Limits::default()
    };
    let want = "out of memory: the program starts with 2 MB (limit 1 MB; raise it with --memory)";
    let mut c = Kio::new(limits).unwrap();
    let e = c.run("fn main() -> Int { 1 }", vec![]).unwrap_err();
    assert_eq!(c.explain(&e).as_deref(), Some(want), "{e:#}");

    let src =
        r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> { res.send(200, [], [], "hi".to_bytes()) }"#;
    let e = kio_host::Server::new(vec![("main".into(), src.into())], limits)
        .err()
        .expect("refused");
    assert_eq!(format!("{e:#}"), want);
}

/// C7a (4): the C7 equality bug's class, a 64-bit slot read through a 32-bit `usize`, is
/// invisible to the runtime's native unit tests. Every runtime operation that takes, returns,
/// stores, orders or displays a slot, here with `Int`s that agree in their low 32 bits
/// (4294967297, 1, -4294967295) and `Float`s whose low 32 bits are all 0 (1.5, 2.5), compiled
/// against the interpreter.
#[test]
fn every_slot_keeps_its_high_bits() {
    let decls = "struct P { a: Int, f: Float }
enum E { A(Int, Float) }
struct L { xs: List<Int>, p: Option<Int> }
struct F { fs: List<Float>, p: Option<Float>, x: Float }
struct M { a: Option<Int>, b: Option<Int>, keys: List<Int>, m: Map<Int, Int> }
struct N { a: Option<Float>, b: Option<Float>, m: Map<Int, Float> }
struct O { a: Int, b: Result<Float, Int>, c: Result<Int, Float>, d: Float, e: Option<Int>, f: Option<Float> }
struct S { p: P, e: E, n: Int, f: Float }
struct T { a: List<Int>, b: Bool, c: Bool, s: Float, t: Int }";
    let cases = [
        // `rt_eq` through each container, and `contains`.
        "List<Bool> { [[4294967297] == [1], [1.5] == [2.5], [4294967297].contains(1), [2.5].contains(1.5)] }",
        "List<Bool> { [Some(4294967297) == Some(1), Some(Some(2.5)) == Some(Some(1.5)), P { a: 1, f: 1.5 } == P { a: 4294967297, f: 1.5 }] }",
        "List<Bool> { let r: Result<Int, Float> = Err(1.5); let s: Result<Int, Float> = Err(2.5); [r == s, E::A(1, 1.5) == E::A(-4294967295, 1.5)] }",
        // A list: push, get, set, `+=`, pop, sort, display.
        "L { let mut xs = [4294967297, 1]; xs.push(-4294967295); xs[1] = xs[0] + 1; xs[2] += 4294967296; let p = xs.pop(); xs.sort(); L { xs: xs, p: p } }",
        "List<Int> { let mut xs = [-4294967295, 4294967297, 1, 8589934593]; xs.sort(); xs }",
        "F { let mut fs = [1.5]; fs.push(2.5); fs[0] = fs[1] + 1.0; let p = fs.pop(); F { x: fs[0], fs: fs, p: p } }",
        // A map: insert, get, remove, keys, equality, display.
        "M { let mut m: Map<Int, Int> = Map::new(); m.insert(4294967297, 1); m.insert(1, 4294967297); m.insert(-4294967295, 2); M { a: m.get(1), b: m.get(4294967297), keys: m.keys(), m: m } }",
        "N { let mut m: Map<Int, Float> = Map::new(); m.insert(4294967297, 1.5); m.insert(1, 2.5); m.remove(4294967297); N { a: m.get(1), b: m.get(4294967297), m: m } }",
        "Bool { let mut m: Map<Int, Int> = Map::new(); m.insert(1, 0); let mut n: Map<Int, Int> = Map::new(); n.insert(4294967297, 0); m == n }",
        // `Option` and `Result` built-ins.
        "O { let o: Option<Int> = None; let r: Result<Float, Int> = Err(4294967297); let s: Result<Int, Float> = Ok(-4294967295); O { a: o.unwrap_or(4294967297), b: Some(2.5).ok_or(4294967297), c: o.ok_or(1.5), d: r.unwrap_or(2.5), e: r.err(), f: Some(1.5).map(|f| f + 1.0).and_then(|f| r.ok().map(|g| g + f)) } }",
        "List<Option<Int>> { let r: Result<Int, Str> = Ok(-4294967295); [r.ok(), Some(r.unwrap_or(1)), Some(4294967297).and_then(|x| Some(x + 1))] }",
        // Fields, payloads, `match`, closures over slots, `for`.
        "S { let p = P { a: 4294967297, f: 2.5 }; let e = E::A(-4294967295, 1.5); let n = match e { E::A(x, _) => x }; S { n: n, f: p.f, p: p, e: e } }",
        "List<Int> { let k = 4294967296; let f = |x: Int| x + k; [1, 4294967297].map(f) }",
        "T { let xs = [4294967297, 1, -4294967295]; let mut s = 0.0; for f in [1.5, 2.5] { s = s + f; } let mut t = 0; for x in xs { t = t + x; } T { a: xs.filter(|x| x == 1), b: xs.any(|x| x == 4294967297), c: xs.all(|x| x != 1), s: s, t: t } }",
    ];
    for case in cases {
        let (ret, body) = case.split_once(' ').unwrap();
        let src = format!("{decls}\nfn main() -> {ret} {body}");
        let r = both(&src);
        assert!(r.is_ok(), "{src}: {r:?}");
    }
}
