//! L32: capability groups. A struct whose fields are all capability types is a capability
//! struct: a parameter only, flattened by lowering (docs/proposals/cap-groups.md).
use lang::ir::LocalId;
use lang::lower::{lower, lower_entry};
use lang::{check, check_files, check_files_for_tests, report, run, CapTable, Caps, Value};
use std::collections::BTreeMap;

mod common;

/// Message, hint and the source the one diagnostic of `src` points at.
fn reject(src: &str) -> (String, String, String) {
    let diags = check(src, &CapTable::v0()).expect_err("should be rejected");
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    (
        d.msg.clone(),
        d.hint.clone().unwrap_or_default(),
        src[d.span.start..d.span.end].to_string(),
    )
}

fn assert_rejected(src: &str, msg: &str, hint: &str, at: &str) {
    assert_eq!(reject(src), (msg.into(), hint.into(), at.into()), "{src}");
}

fn rep(src: &str) -> String {
    report(&check(src, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}\n{src}")))
}

const ENV: &str = "struct Env { d: Dir, c: Clock }\n";

const AS_VALUE: (&str, &str) = ("capability used as a value", "call methods on the parameter itself");

// The rules table.

#[test]
fn rule1_mixed_struct() {
    assert_rejected(
        "struct Env { d: Dir, name: Str } fn main() {}",
        "struct `Env` mixes capabilities and data: `name: Str`",
        "a struct holds capabilities only or no capability; move `name` to a data struct",
        "name: Str",
    );
    // A capability struct holding a nested capability type is mixed too; the nesting is refused.
    assert_rejected(
        "struct Env { d: Dir, xs: List<Dir> } fn main() {}",
        "capability type in a field",
        "capabilities are parameters only",
        "List<Dir>",
    );
}

#[test]
fn rule2_generic_and_nested() {
    assert_rejected(
        "struct Env<T> { d: Dir } fn main() {}",
        "capability struct `Env` cannot be generic",
        "remove `<..>`: a capability struct lists capability types only",
        "T",
    );
    assert_rejected(
        "struct Env { d: Dir } struct Job { e: Env, c: Clock } fn main() {}",
        "capability struct `Job` nests `Env`",
        "list `Env`'s fields in `Job`",
        "e: Env",
    );
    // Declared after its use: the first pass over the types finds it all the same.
    assert_rejected(
        "struct Job { e: Env } struct Env { d: Dir } fn main() {}",
        "capability struct `Job` nests `Env`",
        "list `Env`'s fields in `Job`",
        "e: Env",
    );
}

#[test]
fn rule3_a_group_is_a_capability_everywhere() {
    let only = "capabilities are parameters only";
    let cases = [
        (
            "struct Job { xs: List<Env> } fn main() {}",
            ("capability type in a field", only, "List<Env>"),
        ),
        (
            "struct Job { e: Option<Env>, n: Int } fn main() {}",
            ("capability type in a field", only, "Option<Env>"),
        ),
        (
            "enum E { A(Env) } fn main() {}",
            ("capability type in a payload", only, "Env"),
        ),
        (
            "fn pick(env: Env) -> Env { env } fn main() {}",
            ("cannot return `Env` from `pick`", only, "Env"),
        ),
        (
            "fn f(x: fn(Env)) {} fn main() {}",
            (
                "function values cannot take or return capabilities",
                "pass the capability to a named function instead",
                "fn(Env)",
            ),
        ),
        (
            "fn g(env: Env) {} fn f() { let h = g; } fn main() {}",
            (
                "function values cannot take or return capabilities",
                "pass the capability to a named function instead",
                "g",
            ),
        ),
        (
            "fn id<T>(x: T) -> T { x } fn f(env: Env) { id::<Env>(env); } fn main() {}",
            (
                "`T` cannot be `Env`: capabilities are parameters only",
                "pass the Env to a function whose parameter has type `Env`",
                "Env",
            ),
        ),
        (
            "fn f(env: Env) { let g = || env.d.list(\"\"); } fn main() {}",
            (
                "closure captures capability `env`",
                "use a named function that takes it as a parameter, `env: Env`",
                "env",
            ),
        ),
        (
            "fn f(env: Env) { let x: Env = env; } fn main() {}",
            (
                "capability `env` bound to local `x`",
                "pass `env` directly instead",
                "env",
            ),
        ),
        (
            "fn f(env: Env) { let x: Option<Env> = None; } fn main() {}",
            (
                "capability nested in `Option<Env>`",
                "take `Env` as a parameter",
                "Option<Env>",
            ),
        ),
        (
            "fn h(env: Env) -> Int { 1 } fn f(env: Env) { let z: Env = h(env); } fn main() {}",
            ("capability type in a local", only, "Env"),
        ),
    ];
    for (src, (msg, hint, at)) in cases {
        assert_rejected(&format!("{ENV}{src}"), msg, hint, at);
    }
}

#[test]
fn rule4_a_field_stands_as_receiver_or_argument() {
    assert_rejected(
        &format!("{ENV}fn f(env: Env) {{ let d = env.d; }} fn main() {{}}"),
        "capability `env.d` bound to local `d`",
        "pass `env.d` directly instead",
        "env.d",
    );
    // Row 6: the other escapes of a field.
    let cases = [
        ("fn f(env: Env) { let xs = [env.d]; }", AS_VALUE, "env.d"),
        ("fn f(env: Env) { let o = Some(env.d); }", AS_VALUE, "env.d"),
        (
            "struct J { d: Dir, n: Int } fn f(env: Env) {}",
            (
                "struct `J` mixes capabilities and data: `n: Int`",
                "a struct holds capabilities only or no capability; move `n` to a data struct",
            ),
            "n: Int",
        ),
        (
            "struct J { n: Int } fn f(env: Env) { let j = J { n: env.d }; }",
            AS_VALUE,
            "env.d",
        ),
        (
            "fn f(env: Env) -> Dir { return env.d; }",
            ("cannot return `Dir` from `f`", "capabilities are parameters only"),
            "Dir",
        ),
        (
            "fn f(env: Env) { let g = |x: Str| env.d.read(x); }",
            (
                "closure captures capability `env`",
                "use a named function that takes it as a parameter, `env: Env`",
            ),
            "env",
        ),
        (
            "fn f(env: Env, x: Dir) { env.d = x; }",
            ("cannot assign to `env`, it is not `mut`", "declare it with `let mut`"),
            "env",
        ),
        ("fn f(env: Env) { match env.d { _ => {} } }", AS_VALUE, "env.d"),
        (
            "fn f(env: Env) -> Bool { env.d == env.d }",
            ("cannot compare `Dir` values", "capabilities have no equality"),
            "env.d == env.d",
        ),
        (
            "fn f(env: Env) { env.d.x; }",
            ("no field `x` on Dir", "only structs have fields"),
            "env.d.x",
        ),
        (
            "fn f(env: Env) { env.q.read(\"a\"); }",
            ("no field `q` on Env", "did you mean `c`?"),
            "env.q",
        ),
        (
            "fn f(env: Env) { env.read(\"a\"); }",
            ("no method `read` on Env", "Env has no methods"),
            "env.read(\"a\")",
        ),
    ];
    for (src, (msg, hint), at) in cases {
        assert_rejected(&format!("{ENV}{src} fn main() {{}}"), msg, hint, at);
    }
}

#[test]
fn rule5_a_literal_is_a_whole_argument() {
    let f = "fn f(env: Env) {} ";
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ let e = Env {{ d: input, c: now }}; }}"),
        "capability struct `Env` built outside an argument",
        "write it as the argument: `f(Env { .. }, ..)`",
        "Env { d: input, c: now }",
    );
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ f(Env {{ d: input, ..input }}); }}"),
        "no `..` in a capability struct literal",
        "name every field",
        "input",
    );
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ f(Env {{ d: input }}); }}"),
        "missing field `c` in `Env`",
        "add `c: ..`",
        "Env { d: input }",
    );
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ f(Env {{ d: input, c: now, x: now }}); }}"),
        "no field `x` on Env",
        "did you mean `c`?",
        "now",
    );
    // Row 5: a block or `if` yielding a group is a value, not an argument place.
    assert_rejected(
        &format!("{ENV}{f}fn g(env: Env) {{ let e = env; }} fn main() {{}}"),
        "capability `env` bound to local `e`",
        "pass `env` directly instead",
        "env",
    );
    let cases = [
        "fn g(env: Env) { let xs = [env]; }",
        "fn g(env: Env) { let o = Some(env); }",
        "fn g(env: Env) { match env { e => {} } }",
        "fn g(env: Env) { if let x = env {} }",
        "fn g(env: Env) { assert_eq(env, env); }",
        "fn g(env: Env, e2: Env, c: Bool) { f(if c { env } else { e2 }); }",
        "fn g(env: Env) { let x = { env }; }",
        "fn g(env: Env) { f({ env }); }",
    ];
    for src in cases {
        let (msg, hint, at) = reject(&format!("{ENV}{f}{src} fn main() {{}}"));
        assert_eq!(
            (msg.as_str(), hint.as_str(), at.as_str()),
            (AS_VALUE.0, AS_VALUE.1, "env"),
            "{src}"
        );
    }
    assert_rejected(
        &format!("{ENV}{f}fn g(env: Env, other: Env) {{ env = other; }} fn main() {{}}"),
        "cannot assign to `env`, it is not `mut`",
        "declare it with `let mut`",
        "env",
    );
    assert_rejected(
        &format!("{ENV}{f}fn g() {{ let h = |e: Env| 1; }} fn main() {{}}"),
        "function values cannot take or return capabilities",
        "pass the capability to a named function instead",
        "Env",
    );
}

#[test]
fn rule6_a_literal_value_is_a_capability_place() {
    let f = "fn f(env: Env) {} ";
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ f(Env {{ d: \"s\", c: now }}); }}"),
        "`Env.d` takes a `Dir`, found `Str`",
        "give a capability parameter or a field of one: `d: d`",
        "\"s\"",
    );
    assert_rejected(
        &format!("{ENV}{f}fn main(input: Dir, now: Clock) {{ f(Env {{ d: now, c: now }}); }}"),
        "`Env.d` takes a `Dir`, found `Clock`",
        "give a capability parameter or a field of one: `d: d`",
        "now",
    );
    assert_rejected(
        &format!("{ENV}{f}fn g(env: Env) {{ f(Env {{ d: env, c: env.c }}); }} fn main() {{}}"),
        "`Env.d` takes a `Dir`, found `Env`",
        "give a capability parameter or a field of one: `d: d`",
        "env",
    );
}

#[test]
fn rule7_main_takes_capability_types_only() {
    assert_rejected(
        &format!("{ENV}fn main(env: Env) {{}}"),
        "`main` cannot take capability struct `Env`",
        "main's parameters are the grants: write `d: Dir, c: Clock` and build `Env { .. }` where it is passed",
        "Env",
    );
}

#[test]
fn rule8_test_names_are_distinct() {
    let cases = [
        (
            "#[test] fn t(env: Env, d: Int) {}",
            "test `t` names `d` twice: `env.d` and `d`",
            "d: Int",
        ),
        (
            "#[test] fn t(a: Env, b: Env) {}",
            "test `t` names `d` twice: `a.d` and `b.d`",
            "b: Env",
        ),
        (
            "#[test] fn t(env: Env, d: Dir) {}",
            "test `t` names `d` twice: `env.d` and `d`",
            "d: Dir",
        ),
        (
            "#[test] fn t(d: Dir, env: Env) {}",
            "test `t` names `d` twice: `d` and `env.d`",
            "env: Env",
        ),
    ];
    for (src, msg, at) in cases {
        let src = format!("{ENV}{src}");
        let diags = check_files_for_tests(&[("app", &src)], &CapTable::v0()).expect_err("rejected");
        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(
            (d.msg.as_str(), d.hint.as_deref(), &src[d.span.start..d.span.end]),
            (msg, Some("rename the field or drop the parameter"), at)
        );
    }
    // Distinct leaves check, with each field a grant of its short name.
    let src = format!("{ENV}#[test] fn t(env: Env, n: Int, s: Secret) {{}}");
    let p = check_files_for_tests(&[("app", &src)], &CapTable::v0()).unwrap();
    let t = &p.tests()[0];
    let pair = |n: &str, t: &str| (n.to_string(), t.to_string());
    assert_eq!(
        t.params,
        vec![pair("d", "Dir"), pair("c", "Clock"), pair("s", "Secret")]
    );
    assert_eq!(t.inputs, vec![pair("n", "Int")]);
}

#[test]
fn rule9_the_parameter_limit_counts_fields() {
    let fields = |n: usize| (0..n).map(|i| format!("f{i}: Dir")).collect::<Vec<_>>().join(", ");
    let src = format!(
        "struct A {{ {} }} struct B {{ {} }} fn t(a: A, b: B, n: Int) {{}} fn main() {{}}",
        fields(500),
        fields(499)
    );
    assert_rejected(
        &src,
        "`t` takes 1000 parameters; at most 999",
        "the limit counts a capability struct's fields; take fewer",
        "n: Int",
    );
    let ok = format!(
        "struct A {{ {} }} struct B {{ {} }} fn t(a: A, b: B) {{}} fn main() {{}}",
        fields(500),
        fields(499)
    );
    check(&ok, &CapTable::v0()).unwrap();
}

// Accepted programs.

#[test]
fn accepted_shapes() {
    // Two fields of one type; an empty struct stays data.
    let src = "struct Env { a: Dir, b: Dir } struct Empty {}
fn f(env: Env) -> Result<(), IoErr> { env.b.create(\"x\", env.a.read(\"x\")?) }
fn g(e: Empty) -> Empty { let x = e; x }
fn main(input: Dir, out: Dir) -> Result<(), IoErr> { g(Empty {}); f(Env { a: input, b: out }) }";
    assert_eq!(
        rep(src),
        "f    : env.a read, env.b create\ng    : none\nmain : input read, out create"
    );
    // A generic function taking a group is reported once.
    let src = "struct Env { d: Dir }
fn via<T>(env: Env, x: T) -> T { let _b = env.d.read(\"a\"); x }
fn main(input: Dir) -> Int { via(Env { d: input }, 1) + via::<Str>(Env { d: input }, \"a\").len() }";
    assert_eq!(rep(src), "main : input read\nvia  : env.d read");
}

#[test]
fn pub_struct_across_a_module() {
    let web = "pub struct Env { d: Dir, c: Clock }
pub fn handle(env: Env) -> Int { let _ = env.d.list(\"\"); env.c.now_ms() }
fn helper(env: Env) -> Int { handle(env) }";
    let app = "mod web;
fn main(input: Dir, now: Clock) -> Int { web::handle(web::Env { d: input, c: now }) }";
    let p = check_files(&[("app", app), ("web", web)], &CapTable::v0()).unwrap();
    assert_eq!(
        report(&p),
        "main        : input list, now now_ms\nweb::handle : env.d list, env.c now_ms\nweb::helper : env.d list, env.c now_ms"
    );
    // A private one is refused from the root, as any struct.
    let private = web.replace("pub struct", "struct");
    let diags = check_files(&[("app", app), ("web", &private)], &CapTable::v0()).expect_err("private");
    assert_eq!(diags[0].msg, "`Env` is private to `web`");
    // Row 4: a literal of the wrong module's type.
    let app = "mod web;\nstruct Env { d: Dir, c: Clock }
fn main(input: Dir, now: Clock) -> Int { web::handle(Env { d: input, c: now }) }";
    let diags = check_files(&[("app", app), ("web", web)], &CapTable::v0()).expect_err("wrong type");
    assert_eq!(
        (diags[0].msg.as_str(), diags[0].hint.as_deref()),
        (
            "expected web::Env, found Env in argument 1 of `web::handle`",
            Some("`web::handle` is declared as `fn handle(env: web::Env)`")
        )
    );
}

/// Row 4: a local shadowing the group parameter is data inside its block and the group after it.
#[test]
fn shadowing_follows_the_checked_type() {
    let src = "struct Env { d: Dir } struct Row { d: Int }
fn f(env: Env) -> Int {
  let n = { let env = Row { d: 7 }; env.d };
  let _ = env.d.read(\"a\");
  n
}
fn main(input: Dir) -> Int { f(Env { d: input }) }";
    assert_eq!(rep(src), "f    : env.d read\nmain : input read");
    let bad = src.replace("env.d };", "env.d.read(\"a\") };");
    let (msg, hint, at) = reject(&bad);
    assert_eq!(
        (msg.as_str(), hint.as_str(), at.as_str()),
        ("no method `read` on Int", "Int has: abs, bit_and, bit_or, bit_xor, checked_add, checked_mul, checked_sub, iso_utc, max, min, pow, shl, shr, to_float, to_str", "env.d.read(\"a\")")
    );
}

// The report: function lines with paths, charging through passes, literals and narrowing.

const BOOKING: &str = r#"struct Env { db: Db, admin: Secret, audit: Dir }
struct Writer { db: Db, audit: Dir }
fn handle(env: Env, r: Str) -> Result<Int, DbErr> {
  if env.admin.matches(r) { route(env, r) } else { Ok(0) }
}
fn route(env: Env, r: Str) -> Result<Int, DbErr> {
  let _ = save(Writer { db: env.db, audit: env.audit }, r)?;
  init(env.db)?;
  Ok(1)
}
fn save(w: Writer, r: Str) -> Result<Int, DbErr> { let _ = w.audit.append("log", r.to_bytes()); w.db.exec(r, []) }
fn init(db: Db) -> Result<(), DbErr> { let _ = db.query("select 1", [])?; Ok(()) }
fn main(db: Db, admin: Secret, audit: Dir) -> Result<Int, DbErr> { handle(Env { db: db, admin: admin, audit: audit }, "x") }
"#;

#[test]
fn report_charges_leaves_by_path() {
    let want = "handle : env.db exec query, env.admin matches, env.audit append
init   : db query
main   : db exec query, admin matches, audit append
route  : env.db exec query, env.audit append
save   : w.db exec, w.audit append";
    assert_eq!(rep(BOOKING), want);
    // `main`'s line is byte-identical to the ungrouped program's.
    let flat = r#"fn handle(db: Db, admin: Secret, audit: Dir, r: Str) -> Result<Int, DbErr> {
  if admin.matches(r) { route(db, admin, audit, r) } else { Ok(0) }
}
fn route(db: Db, admin: Secret, audit: Dir, r: Str) -> Result<Int, DbErr> {
  let _ = save(db, audit, r)?;
  init(db)?;
  Ok(1)
}
fn save(db: Db, audit: Dir, r: Str) -> Result<Int, DbErr> { let _ = audit.append("log", r.to_bytes()); db.exec(r, []) }
fn init(db: Db) -> Result<(), DbErr> { let _ = db.query("select 1", [])?; Ok(()) }
fn main(db: Db, admin: Secret, audit: Dir) -> Result<Int, DbErr> { handle(db, admin, audit, "x") }
"#;
    let main_line = |r: String| r.lines().find(|l| l.starts_with("main")).unwrap().to_string();
    assert_eq!(main_line(rep(BOOKING)), main_line(rep(flat)));
}

/// Aliasing (`Env { a: db, b: db }`) prints two leaves; `main` has the union. Two same-typed
/// grants keep their own methods.
#[test]
fn report_aliasing_and_two_grants() {
    let src = "struct Env { a: Db, b: Db }
fn handle(e: Env) -> Result<Int, DbErr> { let _ = e.a.query(\"q\", [])?; e.b.exec(\"x\", []) }
fn main(db: Db) -> Result<Int, DbErr> { handle(Env { a: db, b: db }) }";
    assert_eq!(rep(src), "handle : e.a query, e.b exec\nmain   : db exec query");
    let two = src.replace(
        "fn main(db: Db) -> Result<Int, DbErr> { handle(Env { a: db, b: db }) }",
        "fn main(x: Db, y: Db) -> Result<Int, DbErr> { handle(Env { a: x, b: y }) }",
    );
    assert_eq!(rep(&two), "handle : e.a query, e.b exec\nmain   : x query, y exec");
}

/// Row 7: `Http` verbs through fields and helpers are the leaf program's.
#[test]
fn http_verbs_through_a_group() {
    let group = r#"struct Env { api: Http, c: Clock }
const PUT: Str = "PUT";
fn a(env: Env) -> Result<HttpResponse, HttpErr> { env.api.send(PUT, "/a", [], "".to_bytes(), 0) }
fn b(env: Env) -> Result<HttpResponse, HttpErr> { env.api.send("POST", "/b", [], "".to_bytes(), 0) }
fn c(env: Env, m: Str) -> Result<HttpResponse, HttpErr> { env.api.send(m, "/c", [], "".to_bytes(), 0) }
fn reqs(env: Env) -> List<HttpRequest> { let _ = env.c.now_ms(); [HttpRequest { method: "GET", path: "/", headers: [], body: "".to_bytes(), timeout_ms: 0 }] }
fn d(env: Env) -> List<Result<HttpResponse, HttpErr>> { env.api.send_all(reqs(env)) }
fn main(api: Http, now: Clock, m: Str) { let e = 1; }
"#;
    // `m: Str` is not a grant: give `main` a shape that checks.
    let group = group.replace("fn main(api: Http, now: Clock, m: Str) { let e = 1; }",
        "fn main(api: Http, now: Clock) { let _ = a(Env { api: api, c: now }); let _ = b(Env { api: api, c: now }); let _ = c(Env { api: api, c: now }, \"X\"); let _ = d(Env { api: api, c: now }); }");
    let want = "a    : env.api send PUT
b    : env.api send POST
c    : env.api send *
d    : env.api send_all *, env.c now_ms
main : api send * POST PUT send_all *, now now_ms
reqs : env.c now_ms";
    assert_eq!(rep(&group), want);
}

// Lowering: a group parameter is its fields, in field order; a test's by short name.

#[test]
fn lowering_flattens_groups() {
    let p = check(BOOKING, &CapTable::v0()).unwrap();
    let m = lower(&p, &CapTable::v0());
    let f = |name: &str| m.funcs.iter().find(|f| f.name == name).unwrap();
    let names = |name: &str| {
        let f = f(name);
        f.locals[..f.params as usize]
            .iter()
            .map(|l| l.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("handle"), ["env.db", "env.admin", "env.audit", "r"]);
    assert_eq!(names("route"), ["env.db", "env.admin", "env.audit", "r"]);
    assert_eq!(names("save"), ["w.db", "w.audit", "r"]);
    assert_eq!(names("main"), ["db", "admin", "audit"]);
    // No struct named `Env` or `Writer` exists in the IR.
    assert!(m.structs.iter().all(|s| s.name != "Env" && s.name != "Writer"));
    // `route(env, r)` passes three locals; the narrowing literal picks two of them.
    let route_call = format!("{:?}", f("handle").body);
    assert!(route_call.contains("Local(LocalId(0)), "), "{route_call}");
    let save_call = format!("{:?}", f("route").body);
    assert!(
        save_call.contains(&format!("{:?}", LocalId(0))) && save_call.contains(&format!("{:?}", LocalId(2))),
        "{save_call}"
    );
    // A test entry names its leaves by short name.
    let src = format!("{BOOKING}#[test] fn t(env: Env) {{ let _ = handle(env, \"x\"); }}");
    let p = check_files_for_tests(&[("app", &src)], &CapTable::v0()).unwrap();
    let m = lower_entry(&p, &CapTable::v0(), "t");
    let t = &m.funcs[m.main.0 as usize];
    let names: Vec<_> = t.locals[..t.params as usize].iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, ["db", "admin", "audit"]);
}

// Running: the interpreter sees today's program.

/// Grants `Dir`s by name; records every call.
#[derive(Default)]
struct Stub {
    files: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    calls: Vec<(String, String, String)>,
}

impl Caps for Stub {
    fn has(&self, name: &str, ty: &str) -> bool {
        ty == "Dir" && self.files.contains_key(name)
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let Value::Str(path) = args[0].clone() else {
            return Err("path".into());
        };
        self.calls.push((cap.into(), method.into(), path.to_string()));
        let files = self.files.get_mut(cap).ok_or("not granted")?;
        use lang::ir::{EnumId, EnumRef};
        Ok(match (method, args.get(1)) {
            ("read", None) => match files.get(&*path) {
                Some(b) => Value::Enum(EnumRef::Result, 0, vec![Value::Bytes(b.clone().into())]),
                None => Value::Enum(
                    EnumRef::Result,
                    1,
                    vec![Value::Enum(EnumRef::User(EnumId(0)), 0, vec![])],
                ),
            },
            ("create", Some(Value::Bytes(b))) => {
                files.insert(path.to_string(), b.to_vec());
                Value::Enum(EnumRef::Result, 0, vec![Value::Unit])
            }
            _ => return Err(format!("unsupported `{method}`")),
        })
    }
}

#[test]
fn interpreter_runs_a_group_program() {
    let src = "struct Env { src: Dir, dst: Dir } struct Out { dst: Dir }
fn copy(env: Env, name: Str) -> Result<(), IoErr> { put(Out { dst: env.dst }, name, env.src.read(name)?) }
fn put(o: Out, name: Str, b: Bytes) -> Result<(), IoErr> { o.dst.create(name, b) }
fn main(input: Dir, out: Dir) -> Result<(), IoErr> { copy(Env { src: input, dst: out }, \"a.txt\") }";
    let p = check(src, &CapTable::v0()).unwrap();
    let mut s = Stub::default();
    s.files
        .insert("input".into(), BTreeMap::from([("a.txt".to_string(), b"hi".to_vec())]));
    s.files.insert("out".into(), BTreeMap::new());
    let (r, s) = common::big_stack(move || (run(&p, &mut s), s));
    assert_eq!(r.unwrap(), Value::Enum(lang::ir::EnumRef::Result, 0, vec![Value::Unit]));
    assert_eq!(s.files["out"]["a.txt"], b"hi");
    let calls: Vec<(&str, &str, &str)> = s
        .calls
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    assert_eq!(calls, [("input", "read", "a.txt"), ("out", "create", "a.txt")]);
}
