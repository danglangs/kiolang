use lang::{check_files, report, run, CapTable, Caps, Diag, Value};

mod common;

/// kio-web's core from docs/web-support-todo.md.
const WEB: &str = r#"
pub struct Header { name: Str, value: Str }
pub struct Request { method: Str, path: Str, query: Str, headers: List<Header>, body: Bytes }
pub struct Response { status: Int, headers: List<Header>, body: Bytes }

pub fn request(req: Incoming) -> Request {
  let target = req.target().split("?");
  let names = req.header_names();
  let values = req.header_values();
  let mut hs: List<Header> = [];
  for i in 0..names.len() { hs.push(Header { name: names[i], value: values[i] }); }
  let query = if target.len() > 1 { target[1] } else { "" };
  Request { method: req.method(), path: target[0], query: query, headers: hs, body: req.body() }
}

pub fn send(res: Outgoing, r: Response) -> Result<(), WebErr> {
  let mut names: List<Str> = [];
  let mut values: List<Str> = [];
  for h in r.headers { names.push(h.name); values.push(h.value); }
  res.send(r.status, names, values, r.body)
}

pub fn respond(status: Int, ctype: Str, body: Bytes) -> Response {
  let mut hs: List<Header> = [];
  hs.push(Header { name: "content-type", value: ctype });
  Response { status: status, headers: hs, body: body }
}
pub fn text(status: Int, s: Str) -> Response { respond(status, "text/plain; charset=utf-8", s.to_bytes()) }
pub fn json(status: Int, s: Str) -> Response { respond(status, "application/json", s.to_bytes()) }
pub fn not_found() -> Response { text(404, "not found") }

pub fn route(r: Request, method: Str, pattern: Str) -> Option<Map<Str, Str>> {
  if r.method != method { return None; }
  let want = pattern.split("/");
  let got = r.path.split("/");
  if want.len() != got.len() { return None; }
  let mut params: Map<Str, Str> = Map::new();
  for i in 0..want.len() {
    if want[i].starts_with(":") { params.insert(want[i], got[i]); }
    else if want[i] != got[i] { return None; }
  }
  Some(params)
}

pub fn json_str(s: Str) -> Str {
  let b = s.to_bytes();
  let mut out = "\"".to_bytes();
  for i in 0..b.len() {
    let c = b[i];
    if c == 34 || c == 92 { out.push(92); }
    out.push(c);
  }
  out.push(34);
  out.to_str()
}

pub fn file(files: Dir, name: Str) -> Response {
  match files.read(name) {
    Ok(b) => respond(200, "application/octet-stream", b),
    Err(e) => match e { IoErr::NotFound => not_found(), _ => text(500, "read failed") },
  }
}
"#;

const MAIN: &str = r#"
mod web;

fn main(req: Incoming, res: Outgoing, public: Dir) -> Result<(), WebErr> {
  let r = web::request(req);
  let out = match web::route(r, "GET", "/hello/:name") {
    Some(p) => web::json(200, "{\"hello\": " + web::json_str(p.get(":name").unwrap_or("")) + "}"),
    None => web::file(public, r.path),
  };
  web::send(res, out)
}
"#;

/// Grants one request and one `Dir` with no files; records what `send` is given.
struct Stub {
    target: &'static str,
    sent: Vec<(i64, Vec<u8>)>,
}

impl Caps for Stub {
    fn has(&self, name: &str, ty: &str) -> bool {
        matches!(
            (name, ty),
            ("req", "Incoming") | ("res", "Outgoing") | ("public", "Dir")
        )
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let strs = |v: &[&str]| Value::List(v.iter().map(|s| Value::Str((*s).into())).collect());
        let ok = |v| Value::Enum("Result".into(), 0, vec![v]);
        Ok(match (cap, method) {
            ("req", "method") => Value::Str("GET".into()),
            ("req", "target") => Value::Str(self.target.into()),
            ("req", "header_names") => strs(&["host"]),
            ("req", "header_values") => strs(&["kio"]),
            ("req", "body") => Value::Bytes(vec![].into()),
            ("res", "send") => {
                let (Value::Int(status), Value::Bytes(body)) = (&args[0], &args[3]) else {
                    return Err("bad send".into());
                };
                self.sent.push((*status, body.to_vec()));
                ok(Value::Unit)
            }
            ("public", "read") => Value::Enum("Result".into(), 1, vec![Value::Enum("IoErr".into(), 0, vec![])]),
            _ => return Err(format!("unsupported call `{cap}.{method}`")),
        })
    }
}

fn check(files: &[(&str, &str)]) -> Result<lang::Program, Vec<Diag>> {
    check_files(files, &common::caps())
}

/// The first diagnostic of a program that must not check.
fn diag(files: &[(&str, &str)]) -> Diag {
    check(files).unwrap_err().remove(0)
}

fn fails(files: &[(&str, &str)], msg: &str, hint: &str) {
    let d = diag(files);
    assert_eq!((d.msg.as_str(), d.hint.as_deref()), (msg, Some(hint)), "{files:?}");
}

fn serve(target: &'static str) -> Vec<(i64, Vec<u8>)> {
    let p = check(&[("app", MAIN), ("web", WEB)]).unwrap_or_else(|d| panic!("{d:?}"));
    let mut stub = Stub { target, sent: vec![] };
    let result = common::big_stack(move || (run(&p, &mut stub), stub.sent));
    assert!(result.0.is_ok(), "{:?}", result.0);
    result.1
}

#[test]
fn sketch_runs_across_two_files() {
    let body = br#"{"hello": "bob"}"#.to_vec();
    assert_eq!(serve("/hello/bob?x=1"), [(200, body)]);
    assert_eq!(serve("/other"), [(404, b"not found".to_vec())]);
}

#[test]
fn report_names_module_functions() {
    let p = check(&[("app", MAIN), ("web", WEB)]).unwrap();
    let lines: Vec<_> = report(&p)
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let want = [
        "main : req body header_names header_values method target, res send, public read",
        "web::file : files read",
        "web::json : none",
        "web::json_str : none",
        "web::not_found : none",
        "web::request : req body header_names header_values method target",
        "web::respond : none",
        "web::route : none",
        "web::send : res send",
        "web::text : none",
    ];
    assert_eq!(lines, want);
}

#[test]
fn diag_in_a_module_carries_its_file() {
    let d = diag(&[
        ("app", "mod web; fn main() { web::f(); }"),
        ("web", "pub fn f() -> Int { true }"),
    ]);
    assert_eq!(d.msg, "expected Int, found Bool");
    assert_eq!(d.span.file, 1);
    let d = diag(&[("app", "mod web; fn main() { 1 + true; }"), ("web", "")]);
    assert_eq!(d.span.file, 0);
    // A parse error in a module too.
    let d = diag(&[("app", "mod web; fn main() {}"), ("web", "pub fn f( {}")]);
    assert_eq!(d.span.file, 1);
}

#[test]
fn file_list_errors() {
    fails(
        &[("app", "mod web; fn main() {}")],
        "unknown module `web`",
        "add a file named web.kio",
    );
    fails(
        &[("app", "fn main() {}"), ("web", "")],
        "module `web` is never declared",
        "add `mod web;` to app.kio",
    );
    fails(
        &[("app", "mod web; mod web; fn main() {}"), ("web", "")],
        "duplicate `mod web;`",
        "declare each module once",
    );
    fails(
        &[("app", "mod web; fn main() {}"), ("web", "mod util;")],
        "`mod` is only allowed in the root file",
        "declare `mod util;` in app.kio",
    );
    fails(
        &[("app", "mod web; fn main() {}"), ("web", ""), ("web", "")],
        "duplicate file for module `web`",
        "give each file its own name",
    );
}

#[test]
fn file_names_may_end_in_kio() {
    let files = [
        ("app.kio", "mod web; fn main() -> Int { web::f() }"),
        ("web.kio", "pub fn f() -> Int { 1 }"),
    ];
    assert!(check(&files).is_ok());
}

#[test]
fn private_items_stay_private() {
    let web = "fn f() {} struct S { a: Int } enum E { A } pub fn g() -> Int { 1 }";
    let hint = |kw: &str, n: &str| format!("mark it `pub {kw} {n}` in web.kio");
    for (main, msg, kw, n) in [
        ("mod web; fn main() { web::f(); }", "`f` is private to `web`", "fn", "f"),
        (
            "mod web; fn main() { web::S { a: 1 }; }",
            "`S` is private to `web`",
            "struct",
            "S",
        ),
        (
            "mod web; fn main() { web::E::A; }",
            "`E` is private to `web`",
            "enum",
            "E",
        ),
        (
            "mod web; fn h(s: web::S) {} fn main() {}",
            "`S` is private to `web`",
            "struct",
            "S",
        ),
        (
            "mod web; fn main() { let e = web::g(); match e { web::E::A => {} } }",
            "`E` is private to `web`",
            "enum",
            "E",
        ),
    ] {
        fails(&[("app", main), ("web", web)], msg, &hint(kw, n));
    }
}

#[test]
fn modules_cannot_reach_each_other_or_the_root() {
    let root_hint = "a module can only use its own items; pass what it needs as arguments";
    fails(
        &[
            ("app", "mod web; mod db; fn main() {}"),
            ("web", "fn f() { db::g(); }"),
            ("db", "pub fn g() {}"),
        ],
        "module `web` cannot use `db::g`",
        root_hint,
    );
    let item_hint = "a module cannot use the root's items; define it in this module or pass values as arguments";
    fails(
        &[("app", "mod web; fn r() {} fn main() {}"), ("web", "fn f() { r(); }")],
        "unknown function `web::r`",
        item_hint,
    );
    fails(
        &[
            ("app", "mod web; struct P { a: Int } fn main() {}"),
            ("web", "fn f(p: P) {}"),
        ],
        "unknown type `P`",
        item_hint,
    );
    fails(
        &[("app", "mod web; fn main() {}"), ("web", "fn f() { app::g(); }")],
        "module `web` cannot use `app::g`",
        root_hint,
    );
}

#[test]
fn unknown_names_in_a_module_path() {
    fails(
        &[("app", "mod web; fn main() { util::f(); }"), ("web", "")],
        "unknown module `util`",
        "declare it with `mod util;` in the root file",
    );
    fails(
        &[("app", "mod web; fn main() { web::fx(); }"), ("web", "pub fn f() {}")],
        "unknown function `web::fx`",
        "did you mean `f`?",
    );
    fails(
        &[
            ("app", "mod web; fn h(p: web::P) {} fn main() {}"),
            ("web", "pub struct Q { a: Int }"),
        ],
        "unknown type `web::P`",
        "did you mean `Q`?",
    );
}

#[test]
fn same_names_in_different_modules_are_different_types() {
    let web = "pub struct Point { x: Int } pub fn origin() -> Point { Point { x: 0 } }
               pub enum Kind { A, B } pub fn kind() -> Kind { Kind::B }
               pub fn name(k: Kind) -> Str { match k { Kind::A => \"a\", Kind::B => \"b\" } }";
    let root = "mod web; struct Point { x: Int } enum Kind { A, B }
                fn take(p: Point) -> Int { p.x }";
    // The types do not mix.
    let d = diag(&[
        ("app", &format!("{root} fn main() {{ take(web::origin()); }}")),
        ("web", web),
    ]);
    assert_eq!(d.msg, "expected Point, found web::Point");
    let d = diag(&[
        ("app", &format!("{root} fn main() {{ web::name(Kind::A); }}")),
        ("web", web),
    ]);
    assert_eq!(d.msg, "expected web::Kind, found Kind");
    let main = format!(
        "{root} fn main() -> Bool {{
           let k = match web::kind() {{ web::Kind::A => 1, web::Kind::B => 2 }};
           take(Point {{ x: 5 }}) == 5 && web::origin().x == 0 && k == 2
             && web::name(web::Kind::A) == \"a\" && match Kind::B {{ Kind::A => false, Kind::B => true }}
         }}"
    );
    let p = check(&[("app", &main), ("web", web)]).unwrap_or_else(|d| panic!("{d:?}"));
    struct None_;
    impl Caps for None_ {
        fn has(&self, _: &str, _: &str) -> bool {
            false
        }
        fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
            Err(String::new())
        }
    }
    assert_eq!(run(&p, &mut None_).unwrap(), Value::Bool(true));
}

#[test]
fn a_module_wrong_pattern_enum_is_rejected() {
    let web = "pub enum Kind { A } pub enum Other { A }";
    let d = diag(&[
        (
            "app",
            "mod web; fn main() { match web::Kind::A { web::Other::A => {} } }",
        ),
        ("web", web),
    ]);
    assert_eq!(d.msg, "expected web::Kind, found web::Other");
}

#[test]
fn builtin_types_are_visible_in_modules() {
    let web = "pub fn f(o: Option<Int>, r: Result<Int, IoErr>, m: Map<Str, Int>) -> Int { o.unwrap_or(1) }";
    assert!(check(&[("app", "mod web; fn main() {}"), ("web", web)]).is_ok());
}

#[test]
fn main_must_be_in_the_root() {
    let d = diag(&[("app", "mod web; fn x() {}"), ("web", "pub fn main() {}")]);
    assert_eq!(d.msg, "no `main` function");
}

#[test]
fn single_file_check_is_unchanged() {
    let p = lang::check("fn main() {}", &CapTable::default());
    assert!(p.is_ok());
}
