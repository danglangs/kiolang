//! L32b: `kio check --base` with capability groups. Leaves are matched across versions by path,
//! then by (short name, type); a leaf's origins (the `main` grants bound to it) are compared, and
//! a gained origin exits 3 (docs/proposals/cap-groups.md, the adversarial rows).
use lang::{check, report, CapTable};

/// `lang::report_diff` of two one-file programs.
fn diff(old: &str, new: &str) -> lang::AuthorityDiff {
    let p = |s: &str| check(s, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}\n{s}"));
    lang::report_diff(&p(old), &p(new))
}

fn text(old: &str, new: &str) -> String {
    diff(old, new).text(false)
}

fn rep(src: &str) -> String {
    report(&check(src, &CapTable::v0()).unwrap_or_else(|d| panic!("{d:?}\n{src}")))
}

type Row = (String, String, Vec<String>, Option<Vec<String>>);

fn rows(d: &lang::AuthorityDiff) -> Vec<Row> {
    (d.bindings.iter())
        .map(|b| (b.function.clone(), b.leaf.clone(), b.from.clone(), b.was.clone()))
        .collect()
}

fn row(f: &str, leaf: &str, from: &[&str], was: Option<&[&str]>) -> Row {
    let own = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    (f.to_string(), leaf.to_string(), own(from), was.map(own))
}

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
fn match_leaves_by_path_then_short_name_and_type() {
    let leaves = |xs: &[(&str, &str)]| -> Vec<(String, String)> {
        xs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    };
    let m = |o: &[(&str, &str)], n: &[(&str, &str)]| lang::match_leaves(&leaves(o), &leaves(n));
    let some = |s: &str| Some(s.to_string());
    // Leaf-only: the parameter name.
    assert_eq!(m(&[("d", "Db")], &[("d", "Db")]), [("d".to_string(), some("d"))]);
    // `db: Db` becoming `env.db: Db` is one leaf (pass two); a parameter rename is not.
    assert_eq!(
        m(&[("db", "Db")], &[("env.db", "Db")]),
        [("env.db".to_string(), some("db"))]
    );
    assert_eq!(m(&[("d", "Dir")], &[("dir", "Dir")]), [("dir".to_string(), None)]);
    // Two `db` short names on the new side: nothing matches (row 12).
    assert_eq!(
        m(&[("a_db", "Db"), ("b_db", "Db")], &[("a.db", "Db"), ("b.db", "Db")]),
        [("a.db".to_string(), None), ("b.db".to_string(), None)]
    );
    // A parameter `db` added beside an existing `env.db`: the path matches first (row 11).
    assert_eq!(
        m(&[("env.db", "Db")], &[("env.db", "Db"), ("db", "Db")]),
        [("env.db".to_string(), some("env.db")), ("db".to_string(), None)]
    );
    // Types take part: a `Dir` never matches a `Db` of the same short name.
    assert_eq!(m(&[("x", "Dir")], &[("e.x", "Db")]), [("e.x".to_string(), None)]);
}

/// The proposal's A3 shape (row 2b): grouped against ungrouped is no change, as are a leaf moved
/// out of the group and a group parameter rename; a field rename is `-`/`+` and exits 3, as today.
#[test]
fn grouping_refactor_is_no_change() {
    let flat = r#"fn handle(db: Db, admin: Secret, audit: Dir, r: Str) -> Result<Int, DbErr> {
  if admin.matches(r) { route(db, admin, audit, r) } else { Ok(0) }
}
fn route(db: Db, admin: Secret, audit: Dir, r: Str) -> Result<Int, DbErr> { let _ = save(db, audit, r)?; init(db)?; Ok(1) }
fn save(db: Db, audit: Dir, r: Str) -> Result<Int, DbErr> { let _ = audit.append("log", r.to_bytes()); db.exec(r, []) }
fn init(db: Db) -> Result<(), DbErr> { let _ = db.query("select 1", [])?; Ok(()) }
fn main(db: Db, admin: Secret, audit: Dir) -> Result<Int, DbErr> { handle(db, admin, audit, "x") }
"#;
    assert_eq!(text(flat, BOOKING), "no change in authority");
    assert_eq!(text(BOOKING, flat), "no change in authority");
    let moved = BOOKING
        .replace(
            "struct Env { db: Db, admin: Secret, audit: Dir }",
            "struct Env { db: Db, audit: Dir }",
        )
        .replace(
            "fn handle(env: Env, r: Str)",
            "fn handle(env: Env, admin: Secret, r: Str)",
        )
        .replace(
            "if env.admin.matches(r) { route(env, r) }",
            "if admin.matches(r) { route(env, r) }",
        )
        .replace(
            "handle(Env { db: db, admin: admin, audit: audit }, \"x\")",
            "handle(Env { db: db, audit: audit }, admin, \"x\")",
        );
    assert_eq!(text(BOOKING, &moved), "no change in authority");
    assert_eq!(text(BOOKING, &BOOKING.replace("env", "e")), "no change in authority");
    let field = BOOKING
        .replace(
            "struct Env { db: Db, admin: Secret",
            "struct Env { db: Db, token: Secret",
        )
        .replace("env.admin", "env.token")
        .replace("admin: admin", "token: admin");
    let d = diff(BOOKING, &field);
    assert_eq!(d.grants, Vec::<String>::new());
    assert_eq!(d.funcs, ["+ handle: env.token matches", "- handle: env.admin matches"]);
    assert!(d.added && !d.grant_added && !d.binding_added);
    assert_eq!(
        rows(&d),
        [
            row("handle", "env.token", &["admin"], None),
            row("route", "env.token", &["admin"], None)
        ]
    );
}

/// `--detail` shows the function line of a new field; `users` finds a function through the group.
#[test]
fn detail_and_users_through_a_group() {
    let without = BOOKING
        .replace(
            "struct Env { db: Db, admin: Secret, audit: Dir }",
            "struct Env { db: Db, audit: Dir }",
        )
        .replace(
            "if env.admin.matches(r) { route(env, r) } else { Ok(0) }",
            "route(env, r)",
        )
        .replace(
            "fn main(db: Db, admin: Secret, audit: Dir)",
            "fn main(db: Db, audit: Dir)",
        )
        .replace(
            "Env { db: db, admin: admin, audit: audit }",
            "Env { db: db, audit: audit }",
        );
    let d = diff(&without, BOOKING);
    assert_eq!(d.grants, ["+ admin: Secret (matches)"]);
    assert_eq!(d.funcs, ["+ handle: env.admin matches", "+ main: admin matches"]);
    assert_eq!(
        d.text(false),
        "+ admin: Secret (matches)\nauthority added (exit 3); --detail lists each function"
    );
    assert_eq!(
        d.text(true),
        "+ admin: Secret (matches)\n\n+ handle: env.admin matches\n+ main: admin matches\nauthority added (exit 3)"
    );
    // `route` holds the new field too (it is passed `env` whole), so its leaf is new as well.
    assert_eq!(
        rows(&d),
        [
            row("handle", "env.admin", &["admin"], None),
            row("route", "env.admin", &["admin"], None)
        ]
    );
    let p = check(BOOKING, &CapTable::v0()).unwrap();
    assert_eq!(
        lang::users(&p, "audit", &["append"]),
        ["handle", "main", "route", "save"]
    );
    assert_eq!(lang::users(&p, "db", &["query"]), ["handle", "init", "main", "route"]);
}

/// Rows 1 and 2: two `Db` fields swapped between versions; one grant to two fields.
#[test]
fn swapped_fields_show_binding_lines() {
    let base = "struct Env { a: Db, b: Db }
fn handle(e: Env) -> Result<Int, DbErr> { e.a.exec(\"x\", []) }
fn main(x: Db, y: Db) -> Result<Int, DbErr> { handle(Env { a: x, b: y }) }";
    let swapped = base.replace("Env { a: x, b: y }", "Env { a: y, b: x }");
    let d = diff(base, &swapped);
    assert_eq!(d.grants, ["~ x: Db (-exec)", "~ y: Db (+exec)"]);
    // `handle`'s line is unchanged; only `main`'s own leaves move.
    assert_eq!(d.funcs, ["+ main: y exec", "- main: x exec"]);
    assert_eq!(
        d.text(false),
        "~ x: Db (-exec)\n~ y: Db (+exec)\n\n~ handle: e.a from y (was x)\n~ handle: e.b from x (was y)\nauthority added (exit 3); --detail lists each function"
    );
    assert!(d.added && d.grant_added && d.binding_added);
    let two = "struct Env { a: Db, b: Db }
fn handle(e: Env) -> Result<Int, DbErr> { let _ = e.a.query(\"q\", [])?; e.b.exec(\"x\", []) }
fn main(db: Db, other: Db) -> Result<Int, DbErr> { handle(Env { a: db, b: other }) }";
    let one = two.replace(
        "fn main(db: Db, other: Db) -> Result<Int, DbErr> { handle(Env { a: db, b: other }) }",
        "fn main(db: Db) -> Result<Int, DbErr> { handle(Env { a: db, b: db }) }",
    );
    assert_eq!(
        rep(&one).lines().find(|l| l.starts_with("main")).unwrap(),
        "main   : db exec query"
    );
    assert_eq!(
        text(two, &one),
        "~ db: Db (+exec)\n- other: Db (exec)\n\n~ handle: e.b from db (was other)\nauthority added (exit 3); --detail lists each function"
    );
}

/// Rows 1b, 1c, 1d and 13: leaf-only programs re-pairing same-typed grants.
#[test]
fn leaf_only_rebinding_exits_3_under_the_header() {
    let fg = |calls: &str| {
        format!(
            "fn f(d: Db) -> Result<Int, DbErr> {{ d.exec(\"f\", []) }}
fn g(d: Db) -> Result<Int, DbErr> {{ d.exec(\"g\", []) }}
fn main(x: Db, y: Db) -> Result<Int, DbErr> {{ {calls} }}"
        )
    };
    let d = diff(&fg("let _ = f(x)?; g(y)"), &fg("let _ = f(y)?; g(x)"));
    assert_eq!((d.grants.len(), d.funcs.len()), (0, 0));
    assert_eq!(
        d.text(false),
        "derived  grants unchanged; capability binding changed: f, g\n~ f: d from y (was x)\n~ g: d from x (was y)\nauthority added (exit 3)"
    );
    assert!(d.added && d.binding_added && !d.grant_added);
    // Row 1c: every leaf of `copy` already had both origins: no change at all.
    let copy = |calls: &str| {
        format!(
            "fn copy(src: Dir, dst: Dir) -> Result<(), IoErr> {{ dst.write(\"x\", src.read(\"x\")?) }}
fn main(a: Dir, b: Dir) -> Result<(), IoErr> {{ {calls} }}"
        )
    };
    assert_eq!(
        text(&copy("copy(a, b)?; copy(b, a)"), &copy("copy(b, a)?; copy(a, b)")),
        "no change in authority"
    );
    // Row 1d: a single call swapped moves the grant lines and the bindings.
    assert_eq!(
        text(&copy("copy(a, b)"), &copy("copy(b, a)")),
        "~ a: Dir (+write -read)\n~ b: Dir (+read -write)\n\n~ copy: src from b (was a)\n~ copy: dst from a (was b)\nauthority added (exit 3); --detail lists each function"
    );
    // With `read` on both leaves the grant lines stay and the header explains the exit.
    let read = |calls: &str| {
        format!(
            "fn copy(src: Dir, dst: Dir) -> Result<Int, IoErr> {{ Ok(src.read(\"x\")?.len() + dst.read(\"y\")?.len()) }}
fn main(a: Dir, b: Dir) -> Result<Int, IoErr> {{ {calls} }}"
        )
    };
    assert_eq!(
        text(&read("copy(a, b)"), &read("copy(b, a)")),
        "derived  grants unchanged; capability binding changed: copy\n~ copy: src from b (was a)\n~ copy: dst from a (was b)\nauthority added (exit 3)"
    );
    // A second call site adds an origin: the grant line shows the method, the binding the origin.
    let inspect = |calls: &str| {
        format!(
            "fn inspect(d: Db) -> Result<Int, DbErr> {{ let _ = d.query(\"q\", [])?; Ok(1) }}
fn main(primary: Db, backup: Db) -> Result<Int, DbErr> {{ {calls} }}"
        )
    };
    let (one, two) = (
        inspect("inspect(primary)"),
        inspect("let _ = inspect(primary)?; inspect(backup)"),
    );
    assert_eq!(
        text(&one, &two),
        "~ backup: Db (+query)\n\n~ inspect: d from backup, primary (was primary)\nauthority added (exit 3); --detail lists each function"
    );
    // An origin lost only is shown under `--detail`, and is not an addition.
    let d = diff(&two, &one);
    assert_eq!(d.text(false), "~ backup: Db (-query)\nauthority removed only");
    assert_eq!(
        d.text(true),
        "~ backup: Db (-query)\n\n- main: backup query\n\n~ inspect: d from primary (was backup, primary)\nauthority removed only"
    );
}

/// Row 11: a parameter added beside a field of the same short name is new, never a rebinding.
#[test]
fn added_parameter_beside_a_field_is_new() {
    let base = "struct Env { db: Db }
fn h(env: Env) -> Result<Int, DbErr> { let _ = env.db.query(\"q\", [])?; Ok(1) }
fn main(primary: Db, backup: Db) -> Result<Int, DbErr> { h(Env { db: primary }) }";
    let new = "struct Env { db: Db }
fn x(db: Db) -> Result<Int, DbErr> { db.exec(\"x\", []) }
fn h(env: Env, db: Db) -> Result<Int, DbErr> { let _ = env.db.query(\"q\", [])?; x(db) }
fn main(primary: Db, backup: Db) -> Result<Int, DbErr> { h(Env { db: primary }, backup) }";
    let d = diff(base, new);
    assert_eq!(d.grants, ["~ backup: Db (+exec)"]);
    assert_eq!(d.funcs, ["+ h: db exec", "+ main: backup exec", "+ x: db exec"]);
    assert_eq!(
        rows(&d),
        [row("h", "db", &["backup"], None), row("x", "db", &["backup"], None)]
    );
    assert_eq!(
        d.text(false),
        "~ backup: Db (+exec)\nauthority added (exit 3); --detail lists each function"
    );
}

/// Row 12: a grouping that renames its leaves is `-`/`+`; keeping the leaf names is silent.
#[test]
fn grouping_that_renames_leaves_is_not_silent() {
    let base =
        "fn merge(a_db: Db, b_db: Db) -> Result<Int, DbErr> { let _ = a_db.query(\"q\", [])?; b_db.exec(\"x\", []) }
fn main(x: Db, y: Db) -> Result<Int, DbErr> { merge(x, y) }";
    let grouped = "struct Env { db: Db }
fn merge(a: Env, b: Env) -> Result<Int, DbErr> { let _ = a.db.query(\"q\", [])?; b.db.exec(\"x\", []) }
fn main(x: Db, y: Db) -> Result<Int, DbErr> { merge(Env { db: x }, Env { db: y }) }";
    let d = diff(base, grouped);
    assert_eq!(d.grants, Vec::<String>::new());
    assert_eq!(
        d.funcs,
        ["+ merge: a.db query, b.db exec", "- merge: a_db query, b_db exec"]
    );
    assert!(d.added && !d.binding_added);
    let same_names = "struct A { a_db: Db } struct B { b_db: Db }
fn merge(a: A, b: B) -> Result<Int, DbErr> { let _ = a.a_db.query(\"q\", [])?; b.b_db.exec(\"x\", []) }
fn main(x: Db, y: Db) -> Result<Int, DbErr> { merge(A { a_db: x }, B { b_db: y }) }";
    assert_eq!(text(base, same_names), "no change in authority");
}

/// Row 16: a `main` grant renamed folds every binding line it moves under the grant's pair.
#[test]
fn main_grant_rename_folds_binding_lines() {
    let base = "fn f(d: Db) -> Result<Int, DbErr> { d.exec(\"f\", []) }
fn g(d: Db) -> Result<Int, DbErr> { d.exec(\"g\", []) }
fn main(primary: Db) -> Result<Int, DbErr> { let _ = f(primary)?; g(primary) }";
    let d = diff(base, &base.replace("primary", "main_db"));
    assert_eq!(
        d.text(false),
        "+ main_db: Db (exec)\n- primary: Db (exec)\n  bindings follow the rename: f, g\nauthority added (exit 3); --detail lists each function"
    );
    assert!(!d.binding_added);
    assert_eq!(
        rows(&d),
        [
            row("f", "d", &["main_db"], Some(&["primary"])),
            row("g", "d", &["main_db"], Some(&["primary"]))
        ]
    );
    // A leaf rebound to a grant that is new in `main` is the grant line's `+`: its row is detail.
    let base = "fn f(d: Db) -> Result<Int, DbErr> { d.exec(\"f\", []) }
fn main(primary: Db) -> Result<Int, DbErr> { f(primary) }";
    let new = "fn f(d: Db) -> Result<Int, DbErr> { d.exec(\"f\", []) }
fn main(primary: Db, backup: Db) -> Result<Int, DbErr> { let _ = f(primary)?; f(backup) }";
    let d = diff(base, new);
    assert_eq!(
        d.text(false),
        "+ backup: Db (exec)\nauthority added (exit 3); --detail lists each function"
    );
    assert_eq!(
        d.text(true),
        "+ backup: Db (exec)\n\n+ main: backup exec\n\n~ f: d from backup, primary (was primary)\nauthority added (exit 3)"
    );
    assert!(!d.binding_added);
}
