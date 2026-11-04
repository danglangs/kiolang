use lang::{check, run, Caps, RunErr, Value};
use std::collections::BTreeMap;

mod common;

type Call = (String, String, String);
type Grant<'a> = (&'a str, &'a [(&'a str, &'a [u8])]);

/// Grants `Dir`s by name, each holding files; records `(cap, method, path)` for every call.
#[derive(Default)]
struct StubDir {
    files: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    calls: Vec<Call>,
}

impl Caps for StubDir {
    fn has(&self, name: &str, ty: &str) -> bool {
        ty == "Dir" && self.files.contains_key(name)
    }

    fn call(&mut self, cap: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let Value::Str(path) = args[0].clone() else {
            return Err("path must be Str".to_string());
        };
        self.calls.push((cap.to_string(), method.to_string(), path.clone()));
        if path == "boom" {
            return Err("disk on fire".to_string());
        }
        if path == "liar" {
            return Ok(Value::Int(7));
        }
        let files = self.files.get_mut(cap).ok_or("not granted")?;
        let variant = |en: &str, i, payload| Value::Enum(en.to_string(), i, payload);
        Ok(match (method, args.get(1)) {
            ("read", None) => match files.get(&path) {
                Some(b) => variant("Result", 0, vec![Value::Bytes(b.clone())]),
                None => variant("Result", 1, vec![variant("IoErr", 0, vec![])]),
            },
            ("create", Some(Value::Bytes(b))) => {
                files.insert(path, b.clone());
                variant("Result", 0, vec![Value::Unit])
            }
            _ => return Err(format!("unsupported call `{method}`")),
        })
    }
}

fn stub(grants: &[Grant]) -> StubDir {
    let mut s = StubDir::default();
    for (name, files) in grants {
        let files = files.iter().map(|(p, b)| (p.to_string(), b.to_vec()));
        s.files.insert(name.to_string(), files.collect());
    }
    s
}

fn calls(cs: &[(&str, &str, &str)]) -> Vec<Call> {
    cs.iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect()
}

/// Checks `src` and runs it on the stack `run` documents for debug builds.
fn exec(src: &str, mut s: StubDir) -> (Result<Value, RunErr>, StubDir) {
    let p = check(src, &common::caps()).unwrap_or_else(|d| panic!("{d:?}"));
    let t = std::thread::Builder::new().stack_size(8 << 20);
    t.spawn(move || (run(&p, &mut s), s)).unwrap().join().unwrap()
}

const COPY: &str = "
fn load(d: Dir, name: Str) -> Result<Bytes, IoErr> { d.read(name) }
fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
    let b = load(input, \"a.txt\")?;
    out.create(\"b.txt\", b)
}";

#[test]
fn helper_reads_and_main_writes() {
    let (r, s) = exec(COPY, stub(&[("input", &[("a.txt", b"hi")]), ("out", &[])]));
    assert_eq!(r.unwrap(), Value::Enum("Result".into(), 0, vec![Value::Unit]));
    assert_eq!(
        s.calls,
        calls(&[("input", "read", "a.txt"), ("out", "create", "b.txt")])
    );
    assert_eq!(s.files["out"]["b.txt"], b"hi");
}

#[test]
fn missing_grant_runs_nothing() {
    for grants in [stub(&[("input", &[])]), stub(&[("input", &[]), ("other", &[])])] {
        let (r, s) = exec(COPY, grants);
        assert_eq!(r.unwrap_err().msg, "no grant for `out: Dir`");
        assert!(s.calls.is_empty());
    }
}

#[test]
fn mistyped_grant_is_refused() {
    let src = "fn main(h: Http) {}";
    let (r, _) = exec(src, stub(&[("h", &[])]));
    assert_eq!(r.unwrap_err().msg, "no grant for `h: Http`");
}

#[test]
fn rejected_program_makes_no_calls() {
    let src = "fn main(input: Dir) { let d = input; d.read(\"a.txt\"); }";
    assert!(check(src, &common::caps()).is_err());
}

#[test]
fn invalid_utf8_is_a_runtime_error() {
    let src = "fn main(d: Dir) -> Result<(), IoErr> { let s = d.read(\"x\")?.to_str(); Ok(()) }";
    let (r, s) = exec(src, stub(&[("d", &[("x", &[0xff, 0xfe])])]));
    let e = r.unwrap_err();
    assert_eq!(
        (e.msg.as_str(), &src[e.span.start..e.span.end]),
        ("invalid UTF-8", "d.read(\"x\")?.to_str()")
    );
    assert_eq!(s.calls.len(), 1);
}

#[test]
fn try_propagates_not_found() {
    let src = "fn main(d: Dir) -> Result<(), IoErr> { d.read(\"missing\")?; d.create(\"no\", \"x\".to_bytes()) }";
    let (r, s) = exec(src, stub(&[("d", &[])]));
    let not_found = Value::Enum("IoErr".into(), 0, vec![]);
    assert_eq!(r.unwrap(), Value::Enum("Result".into(), 1, vec![not_found]));
    assert_eq!(s.calls, calls(&[("d", "read", "missing")]));
}

#[test]
fn host_error_stops_the_run() {
    let src = "fn main(d: Dir) { d.read(\"boom\"); d.read(\"after\"); }";
    let (r, s) = exec(src, stub(&[("d", &[])]));
    let e = r.unwrap_err();
    assert_eq!(
        (e.msg.as_str(), &src[e.span.start..e.span.end]),
        ("disk on fire", "d.read(\"boom\")")
    );
    assert_eq!(s.calls, calls(&[("d", "read", "boom")]));
}

#[test]
fn malformed_host_value_stops_the_run() {
    let src = "fn main(d: Dir) { d.read(\"liar\"); d.read(\"after\"); }";
    let (r, s) = exec(src, stub(&[("d", &[])]));
    assert_eq!(r.unwrap_err().msg, "host returned a malformed value for `read`");
    assert_eq!(s.calls, calls(&[("d", "read", "liar")]));
}
