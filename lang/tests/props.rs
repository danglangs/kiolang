//! T2a: property tests: generable test parameters, `assume`, and `lang::prop`.
use lang::eval::{run_module, DISCARDED};
use lang::ir::{Module, Ty};
use lang::lower::lower_entry;
use lang::prop::{case_entry, debug_literal, generate, inputs, minimize, shrink, size_of_case, Rng};
use lang::{check, Caps, RunErr, Value};

mod common;

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities granted".to_string())
    }
}

fn rejects(src: &str, msg: &str, hint: &str, at: &str) {
    let diags = check(src, &common::caps()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    let found = (
        d.msg.as_str(),
        d.hint.as_deref().unwrap_or(""),
        &src[d.span.start..d.span.end],
    );
    assert_eq!(found, (msg, hint, at), "{src}");
}

/// The lowered program of `src` with test `t` as its entry.
fn entry(src: &str, t: &str) -> Module {
    let p = check(src, &common::caps()).unwrap_or_else(|d| panic!("{src}\n{d:?}"));
    lower_entry(&p, &common::caps(), t)
}

/// The module and type of `x` in `#[test] fn t(x: <ty>) {}`, after `decls`.
fn typed(decls: &str, ty: &str) -> (Module, Ty) {
    let m = entry(&format!("{decls}\n#[test]\nfn t(x: {ty}) {{}}\nfn main() {{}}"), "t");
    let ty = inputs(&m)[0].1.clone();
    (m, ty)
}

/// The inputs of cases `0..n` at `seed`, as `kio test` makes them.
fn cases(m: &Module, ty: &Ty, seed: u64, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| generate(m, ty, &mut Rng::for_case(seed, i as u64), size_of_case(i)))
        .collect()
}

/// Runs `m`'s test entry on `args` in the interpreter.
fn run_case(m: &Module, args: &[Value]) -> Result<Value, RunErr> {
    let mut m = m.clone();
    case_entry(&mut m, args);
    common::big_stack(move || run_module(&m, &mut NoCaps))
}

#[test]
fn a_test_takes_data_and_capabilities() {
    let src = "struct O { id: Str, n: Int }\nenum E { A, B(Int) }\n#[test]\nfn t(d: Dir, xs: List<Int>, o: O, e: Option<E>, m: Map<Str, Bytes>) {}\nfn main() {}";
    let p = check(src, &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    let t = &p.tests()[0];
    assert_eq!(t.params, [("d".to_string(), "Dir".to_string())]);
    let names: Vec<_> = t.inputs.iter().map(|(n, ty)| format!("{n}: {ty}")).collect();
    assert_eq!(names, ["xs: List<Int>", "o: O", "e: Option<E>", "m: Map<Str, Bytes>"]);
    // Inputs are not grants: the report's line for the test names only `d`.
    assert_eq!(
        lang::entry_authority(&p, "t").main,
        [("d".to_string(), "Dir".to_string())]
    );
}

#[test]
fn a_test_cannot_take_a_function() {
    let hint = "build it inside the test";
    rejects(
        "#[test]\nfn t(f: fn(Int) -> Bool) {}\nfn main() {}",
        "a test cannot take `fn(Int) -> Bool`",
        hint,
        "f: fn(Int) -> Bool",
    );
    rejects(
        "struct S { f: fn() -> Int }\n#[test]\nfn t(s: List<S>) {}\nfn main() {}",
        "a test cannot take `List<S>`",
        hint,
        "s: List<S>",
    );
    rejects(
        "#[test]\nfn t<T>(x: T) {}\nfn main() {}",
        "a test cannot be generic",
        "remove `<..>`: `kio test` calls a test with no type arguments",
        "T",
    );
}

#[test]
fn assume_rules() {
    rejects(
        "fn main() { assume(true); }",
        "`assume` is only for tests",
        "skip the work with `if`, or move the check into a `#[test]` function",
        "assume(true)",
    );
    rejects(
        "fn f(n: Int) { assume(n > 0); }\n#[test]\nfn t(n: Int) { f(n); }\nfn main() {}",
        "`assume` is only for tests",
        "skip the work with `if`, or move the check into a `#[test]` function",
        "assume(n > 0)",
    );
    rejects(
        "#[test]\nfn t(n: Int) { assume(n, 1); }\nfn main() {}",
        "`assume` takes 1 argument, found 2",
        "`assume` is declared as `fn assume(cond: Bool)`",
        "assume(n, 1)",
    );
    rejects(
        "#[test]\nfn t(n: Int) { assume(n); }\nfn main() {}",
        "expected Bool, found Int",
        "",
        "n",
    );
    rejects(
        "fn assume(b: Bool) {}\nfn main() {}",
        "`assume` is built in",
        "choose another name",
        "fn assume(b: Bool) {}",
    );
}

#[test]
fn assume_discards_a_case() {
    let src = "#[test]\nfn t(n: Int) { assume(n > 0); assert(n > 0, \"positive\"); }\nfn main() {}";
    let m = entry(src, "t");
    let e = run_case(&m, &[Value::Int(-3)]).unwrap_err();
    assert_eq!(
        (e.msg.as_str(), &src[e.span.start..e.span.end]),
        (DISCARDED, "assume(n > 0)")
    );
    assert_eq!(run_case(&m, &[Value::Int(3)]), Ok(Value::Unit));
}

#[test]
fn a_case_runs_with_its_values_and_capabilities() {
    let src = "struct P { a: Int, b: Str }\nenum E { X, Y(List<Int>) }\n#[test]\nfn t(d: Dir, p: P, e: E, m: HashMap<Int, Bytes>, f: Float) {\n  assert_eq(p, P { a: -9223372036854775807 - 1, b: \"é\" });\n  assert_eq(e, E::Y([1, 2]));\n  assert_eq(m.get(7), Some(\"a\".to_bytes()));\n  assert(f.is_nan(), \"nan\");\n}\nfn main() {}";
    let m = entry(src, "t");
    let tys: Vec<Ty> = inputs(&m).into_iter().map(|(_, t)| t).collect();
    let (Ty::Struct(p), Ty::Enum(e)) = (&tys[0], &tys[1]) else {
        panic!("{tys:?}")
    };
    let mut map = lang::hmap::HashMap::new();
    map.insert(lang::Key::Int(7), Value::Bytes(vec![b'a'].into()));
    let args = [
        Value::Struct(*p, vec![Value::Int(i64::MIN), Value::Str("é".into())]),
        Value::Enum(
            lang::ir::EnumRef::User(*e),
            1,
            vec![Value::List(vec![Value::Int(1), Value::Int(2)].into())],
        ),
        Value::HashMap(map.into()),
        Value::Float(f64::NAN),
    ];
    let mut case = m.clone();
    case_entry(&mut case, &args);
    // The case's entry takes the test's capabilities by name, and only those.
    let main = &case.funcs[case.main.0 as usize];
    assert_eq!((main.params, main.locals[0].name.as_str()), (1, "d"));
    struct OneDir;
    impl Caps for OneDir {
        fn has(&self, name: &str, ty: &str) -> bool {
            (name, ty) == ("d", "Dir")
        }
        fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
            Err("not called".into())
        }
    }
    let r = common::big_stack(move || run_module(&case, &mut OneDir));
    assert_eq!(r, Ok(Value::Unit));
}

#[test]
fn generation_covers_each_type() {
    let (m, ty) = typed("", "Int");
    let ints = cases(&m, &ty, 1, 100);
    for b in [0, 1, -1, i64::MIN, i64::MAX] {
        assert!(ints.contains(&Value::Int(b)), "{b} missing");
    }
    assert!(ints
        .iter()
        .any(|v| matches!(v, Value::Int(n) if n.abs() > 1_000_000 && *n != i64::MIN)));
    let (m, ty) = typed("", "Float");
    let floats: Vec<f64> = (cases(&m, &ty, 1, 100).into_iter())
        .map(|v| if let Value::Float(x) = v { x } else { panic!() })
        .collect();
    assert!(floats.iter().any(|x| x.is_nan()));
    for special in [0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            floats.iter().any(|x| x.to_bits() == special.to_bits()),
            "{special:?} missing"
        );
    }
    assert!(floats.iter().any(|x| x.is_finite() && x.fract() != 0.0));
    let (m, ty) = typed("", "Str");
    let strs: Vec<String> = (cases(&m, &ty, 1, 100).into_iter())
        .map(|v| if let Value::Str(s) = v { s.to_string() } else { panic!() })
        .collect();
    assert_eq!(strs[0], "", "case 0 has size 0");
    assert!(strs.iter().any(|s| s.chars().any(|c| c.len_utf8() > 1)));
    assert!(strs.iter().all(|s| s.chars().all(|c| !c.is_control())));
    assert!(strs.iter().all(|s| s.chars().count() <= 32));
    assert!(strs.iter().any(|s| s.chars().count() > 20));
    let (m, ty) = typed("", "Bytes");
    let bytes: Vec<u8> = (cases(&m, &ty, 1, 100).into_iter())
        .flat_map(|v| if let Value::Bytes(b) = v { b.to_vec() } else { panic!() })
        .collect();
    assert!(bytes.iter().any(|b| *b >= 0x80) && bytes.iter().any(|b| *b < 0x20));
    let (m, ty) = typed("", "List<Int>");
    let lens: Vec<usize> = (cases(&m, &ty, 1, 100).into_iter())
        .map(|v| if let Value::List(xs) = v { xs.len() } else { panic!() })
        .collect();
    assert!(
        lens[..5].iter().all(|&n| n <= 4),
        "lengths grow with the case index: {lens:?}"
    );
    assert!(lens.iter().all(|&n| n <= 32) && lens.iter().any(|&n| n > 20));
    for (ty, want) in [("Map<Str, Int>", "Map"), ("HashMap<Int, Bool>", "HashMap")] {
        let (m, ty) = typed("", ty);
        let vs = cases(&m, &ty, 1, 100);
        let len = |v: &Value| match v {
            Value::Map(x) => x.len(),
            Value::HashMap(x) => x.len(),
            _ => panic!("{want}"),
        };
        assert!(vs.iter().any(|v| len(v) > 5), "{want}");
    }
    let (m, ty) = typed("", "Result<Option<Bool>, ()>");
    let vs = cases(&m, &ty, 1, 100);
    for lit in ["Ok(None)", "Ok(Some(true))", "Ok(Some(false))", "Err(())"] {
        assert!(vs.iter().any(|v| debug_literal(&m, v) == lit), "{lit} missing");
    }
    let (m, ty) = typed("struct P { a: Int, b: Bool }\nenum E { A, B(P), C(Str, Int) }", "E");
    let vs = cases(&m, &ty, 1, 100);
    for prefix in ["E::A", "E::B(P { a: ", "E::C(\""] {
        assert!(
            vs.iter().any(|v| debug_literal(&m, v).starts_with(prefix)),
            "{prefix} missing"
        );
    }
}

/// Container nesting of `v`.
fn depth(v: &Value) -> usize {
    match v {
        Value::List(xs) => 1 + xs.iter().map(depth).max().unwrap_or(0),
        Value::Enum(_, _, vs) | Value::Struct(_, vs) => vs.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[test]
fn recursive_types_are_bounded() {
    let (m, ty) = typed("enum Tree { Leaf(Int), Node(List<Tree>) }", "Tree");
    let vs = cases(&m, &ty, 7, 200);
    assert!(
        vs.iter().all(|v| depth(v) <= 4),
        "{}",
        vs.iter().map(depth).max().unwrap()
    );
    assert!(vs.iter().any(|v| depth(v) >= 2));
    // At the depth bound only a variant that does not hold the enum is chosen.
    let (m, ty) = typed("enum T { More(List<T>), Done }", "List<List<List<List<T>>>>");
    for v in cases(&m, &ty, 3, 100) {
        assert!(
            !debug_literal(&m, &v).contains("[[[[T::More"),
            "{}",
            debug_literal(&m, &v)
        );
    }
}

#[test]
fn generation_is_deterministic_per_seed() {
    let (m, ty) = typed("struct O { id: Str, xs: List<Float>, m: Map<Int, Bytes> }", "List<O>");
    let a: Vec<String> = cases(&m, &ty, 42, 50).iter().map(|v| debug_literal(&m, v)).collect();
    let b: Vec<String> = cases(&m, &ty, 42, 50).iter().map(|v| debug_literal(&m, v)).collect();
    let c: Vec<String> = cases(&m, &ty, 43, 50).iter().map(|v| debug_literal(&m, v)).collect();
    assert_eq!(a, b);
    assert_ne!(a, c);
    // A case does not depend on how many cases run.
    assert_eq!(
        a[..10],
        cases(&m, &ty, 42, 10)
            .iter()
            .map(|v| debug_literal(&m, v))
            .collect::<Vec<_>>()[..]
    );
    // A `Case` carries the values to another thread unchanged.
    let vs = cases(&m, &ty, 42, 50);
    let back = lang::prop::Case::new(&vs).values();
    let shown: Vec<String> = back.iter().map(|v| debug_literal(&m, v)).collect();
    assert_eq!(shown, a);
}

/// The first failing case of `t` in `src` at seed 1, shrunk in the interpreter.
fn shrunk(src: &str) -> (String, usize) {
    let m = entry(src, "t");
    let tys: Vec<Ty> = inputs(&m).into_iter().map(|(_, t)| t).collect();
    let fails = |args: &[Value]| matches!(run_case(&m, args), Err(e) if e.msg != DISCARDED);
    let first = (0..100)
        .map(|i| {
            let mut r = Rng::for_case(1, i);
            tys.iter()
                .map(|t| generate(&m, t, &mut r, size_of_case(i as usize)))
                .collect::<Vec<_>>()
        })
        .find(|args| fails(args))
        .expect("a failing case");
    let (case, runs) = minimize(&m, &tys, first, 1000, fails);
    let shown: Vec<String> = case.iter().map(|v| debug_literal(&m, v)).collect();
    (shown.join(", "), runs)
}

#[test]
fn shrinking_reaches_known_minima() {
    let (case, runs) = shrunk("#[test]\nfn t(n: Int) { assert(n <= 5, \"small\"); }\nfn main() {}");
    assert_eq!(case, "6");
    assert!(runs < 200, "{runs}");
    let (case, _) = shrunk("#[test]\nfn t(xs: List<Int>) { assert(xs.len() <= 2, \"short\"); }\nfn main() {}");
    assert_eq!(case, "[0, 0, 0]");
    let (case, _) = shrunk("#[test]\nfn t(s: Str, n: Int) { assert(s.len() < 3 || n < 10, \"x\"); }\nfn main() {}");
    assert_eq!(case, "\"aaa\", 10");
    let (case, _) = shrunk(
        "struct O { id: Str, total: Int }\n#[test]\nfn t(o: Option<O>) { if let Some(o) = o { assert(o.total >= 0, \"neg\"); } }\nfn main() {}",
    );
    assert_eq!(case, "Some(O { id: \"\", total: -1 })");
    // A shrink that keeps failing stops at the run budget.
    let m = entry("#[test]\nfn t(n: Int) {}\nfn main() {}", "t");
    let (_, runs) = minimize(&m, &[Ty::Int], vec![Value::Int(i64::MAX)], 10, |a| {
        a[0] != Value::Int(0)
    });
    assert_eq!(runs, 10);
}

#[test]
fn shrink_candidates_are_smaller_and_first_is_smallest() {
    let (m, ty) = typed("", "Int");
    assert_eq!(shrink(&m, &ty, &Value::Int(0)), []);
    assert_eq!(shrink(&m, &ty, &Value::Int(8)), [0, 4, 6, 7].map(Value::Int));
    assert_eq!(shrink(&m, &ty, &Value::Int(-4)), [0, 4, -2, -3].map(Value::Int));
    assert_eq!(shrink(&m, &ty, &Value::Int(i64::MIN))[0], Value::Int(0));
    let (m, ty) = typed("", "Float");
    assert_eq!(shrink(&m, &ty, &Value::Float(f64::NAN)), [Value::Float(0.0)]);
    assert_eq!(shrink(&m, &ty, &Value::Float(2.5)), [0.0, 2.0, 1.25].map(Value::Float));
    assert_eq!(shrink(&m, &ty, &Value::Float(0.0)), []);
    let (m, ty) = typed("enum E { A, B(Int) }", "E");
    let Ty::Enum(e) = ty else { panic!() };
    let b = Value::Enum(lang::ir::EnumRef::User(e), 1, vec![Value::Int(12)]);
    let shown: Vec<String> = shrink(&m, &ty, &b).iter().map(|v| debug_literal(&m, v)).collect();
    assert_eq!(shown, ["E::A", "E::B(0)", "E::B(6)", "E::B(9)", "E::B(11)"]);
}

#[test]
fn values_show_as_literals() {
    let (m, ty) = typed("struct Order { id: Str, total: Int }", "Order");
    let Ty::Struct(s) = ty else { panic!() };
    let o = Value::Struct(s, vec![Value::Str("a\"\n".into()), Value::Int(3)]);
    assert_eq!(debug_literal(&m, &o), "Order { id: \"a\\\"\\n\", total: 3 }");
    let show = |v: Value| debug_literal(&m, &v);
    assert_eq!(show(Value::Int(i64::MIN)), "-9223372036854775807 - 1");
    assert_eq!(show(Value::Float(1.0)), "1.0");
    assert_eq!(show(Value::Float(-0.0)), "-0.0");
    assert_eq!(show(Value::Float(1e300)), "1.0e300");
    assert_eq!(show(Value::Float(f64::NAN)), "0.0 / 0.0");
    assert_eq!(show(Value::Float(f64::NEG_INFINITY)), "-1.0 / 0.0");
    assert_eq!(show(Value::Bytes(b"hi".to_vec().into())), "\"hi\".to_bytes()");
    assert_eq!(
        show(Value::Bytes(vec![0, 255].into())),
        "\"00ff\".from_hex().unwrap_or(\"\".to_bytes())"
    );
    let some = Value::Enum(lang::ir::EnumRef::Option, 1, vec![Value::Int(1)]);
    assert_eq!(
        show(Value::List(
            vec![some, Value::Enum(lang::ir::EnumRef::Option, 0, vec![])].into()
        )),
        "[Some(1), None]"
    );
}

/// `Pair<..<Int>>`, `depth` deep.
fn pairs(depth: usize) -> String {
    (0..depth).fold("Int".to_string(), |t, _| format!("Pair<{t}>"))
}

#[test]
fn a_parameter_has_a_bounded_size() {
    let src = |ty: &str| {
        format!("struct Pair<T> {{ a: T, b: T }}\nstruct O {{ a: Int, b: Str, c: Float, d: Bool, e: Int, f: Str, g: Int, h: Option<Int>, i: Bytes, j: Int }}\nenum Tree {{ Leaf(Int), Node(List<Tree>) }}\n#[test]\nfn t(p: {ty}) {{}}\nfn main() {{}}")
    };
    let at = format!("p: {}", pairs(12));
    let too_many =
        |n: u64| format!("`t`'s data parameters have up to {n} values together; a property may have at most 4096");
    let hint = "build large values inside the test";
    rejects(&src(&pairs(12)), &too_many(8191), hint, &at);
    for ok in [
        pairs(11),
        "List<O>".into(),
        "List<List<O>>".into(),
        "Tree".into(),
        "Map<Str, List<Int>>".into(),
    ] {
        check(&src(&ok), &common::caps()).unwrap_or_else(|d| panic!("{ok}: {d:?}"));
    }
    // One budget for every data parameter of a test, capabilities aside.
    let two = format!(
        "struct Pair<T> {{ a: T, b: T }}\n#[test]\nfn t(d: Dir, p: {}, q: {}) {{}}\nfn main() {{}}",
        pairs(11),
        pairs(10)
    );
    rejects(&two, &too_many(4095 + 2047), hint, &format!("q: {}", pairs(10)));
    let many = |n: usize| {
        let ps = (0..n)
            .map(|i| format!("x{i}: List<List<Int>>"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("#[test]\nfn t({ps}) {{}}\nfn main() {{}}")
    };
    check(&many(14), &common::caps()).unwrap();
    rejects(&many(15), &too_many(15 * 289), hint, "x14: List<List<Int>>");
    // A `Bytes` counts its bytes, as a `List<Int>` does: 1 + 32.
    let bytes = |n: usize| {
        let fs = (0..n).map(|i| format!("b{i}: Bytes")).collect::<Vec<_>>().join(", ");
        format!("struct B {{ {fs} }}\n#[test]\nfn t(b: B) {{}}\nfn main() {{}}")
    };
    check(&bytes(124), &common::caps()).unwrap();
    rejects(&bytes(125), &too_many(1 + 125 * 33), hint, "b: B");
}

#[test]
fn a_parameter_needs_a_value() {
    let src = |ty: &str| {
        format!("enum Never {{}}\nenum Gone {{ A(Never), B(Int, Never) }}\nenum Some1 {{ A(Never), B(Int) }}\n#[test]\nfn t(x: {ty}) {{}}\nfn main() {{}}")
    };
    let hint = "build it inside the test";
    rejects(
        &src("Never"),
        "a test cannot take `Never`: it has no values",
        hint,
        "x: Never",
    );
    rejects(
        &src("Gone"),
        "a test cannot take `Gone`: it has no values",
        hint,
        "x: Gone",
    );
    for ty in [
        "Option<Never>",
        "List<Never>",
        "Map<Int, Gone>",
        "Result<Int, Never>",
        "Some1",
    ] {
        let empty = if ty.contains("Gone") { "Gone" } else { "Never" };
        let msg = format!("a test cannot take `{ty}`: `{empty}` has no values");
        rejects(&src(ty), &msg, hint, &format!("x: {ty}"));
    }
}

#[test]
fn shrinking_is_lazy() {
    // A 4,095-value input: each candidate is made only when taken, so ten runs cost ten copies.
    let (m, ty) = typed("struct Pair<T> { a: T, b: T }", &pairs(11));
    let v = cases(&m, &ty, 1, 40).pop().unwrap();
    let (_, runs) = minimize(&m, &[ty], vec![v], 10, |_| true);
    assert_eq!(runs, 10);
}
