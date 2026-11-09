//! Each case runs kio source in the interpreter, copies the resulting value into runtime
//! objects, and checks that `display` prints what `Program::display` does.

use crate::desc::{self, Desc, Name, Slice, Table, Variant};
use crate::display::display;
use crate::obj::{self, from_slot, to_slot, Header, Key, Map};
use crate::tests::lock;
use alloc::string::String;
use alloc::vec::Vec;
use lang::{check_files, run, CapTable, Caps, Value};

const LIST_STR: u32 = 5;
const MAP_STR_INT: u32 = 6;
const MAP_INT_STR: u32 = 7;
const PT: u32 = 8; // web::Pt { x: Int, y: Str }
const EMPTY: u32 = 9; // struct E {}
const SHAPE: u32 = 10; // Unit | Circle(Int) | Pair(Int, Str)
const OPT_INT: u32 = 11;
const RES: u32 = 12; // Result<Option<Int>, Str>
const LIST_PT: u32 = 13;
const CHAIN: u32 = 14; // End | Link(Chain)
const LIST_BYTES: u32 = 15;
const LIST_INT: u32 = 16;
const LIST_LIST: u32 = 17;
const DEEP: u32 = 18; // List<DEEP>

/// Where `s` is in `NAMES`.
const fn name(s: &str) -> Name {
    let (h, n) = (NAMES, s.as_bytes());
    let mut i = 0;
    while i + n.len() <= h.len() {
        let mut j = 0;
        while j < n.len() && h[i + j] == n[j] {
            j += 1;
        }
        if j == n.len() {
            return Name {
                off: i as u32,
                len: n.len() as u32,
            };
        }
        i += 1;
    }
    panic!("name not in NAMES")
}

const NAMES: &[u8] = b"web::PtxyEShapeUnitCircleLinkPairOptionNoneSomeResultOkErrChainEndLink";
const NONE: Name = Name { off: 0, len: 0 };

const fn variant(n: &str, start: u32, len: u32) -> Variant {
    Variant {
        name: name(n),
        slots: Slice { start, len },
    }
}

const DESCS: [Desc; 19] = [
    Desc::Unit,
    Desc::Int,
    Desc::Bool,
    Desc::Str,
    Desc::Bytes,
    Desc::List { elem: desc::STR },
    Desc::Map {
        key: desc::STR,
        value: desc::INT,
    },
    Desc::Map {
        key: desc::INT,
        value: desc::STR,
    },
    Desc::Record {
        name: name("web::Pt"),
        slots: Slice { start: 0, len: 2 },
    },
    Desc::Record {
        name: name("E"),
        slots: Slice { start: 0, len: 0 },
    },
    Desc::Enum {
        name: name("Shape"),
        variants: Slice { start: 0, len: 3 },
    },
    Desc::Enum {
        name: name("Option"),
        variants: Slice { start: 3, len: 2 },
    },
    Desc::Enum {
        name: name("Result"),
        variants: Slice { start: 5, len: 2 },
    },
    Desc::List { elem: PT },
    Desc::Enum {
        name: name("Chain"),
        variants: Slice { start: 7, len: 2 },
    },
    Desc::List { elem: desc::BYTES },
    Desc::List { elem: desc::INT },
    Desc::List { elem: LIST_INT },
    Desc::List { elem: DEEP },
];
const SLOTS: [u32; 9] = [
    desc::INT,
    desc::STR,
    desc::INT,
    desc::INT,
    desc::STR,
    desc::INT,
    OPT_INT,
    desc::STR,
    CHAIN,
];
const FIELDS: [Name; 9] = [name("x"), name("y"), NONE, NONE, NONE, NONE, NONE, NONE, NONE];
const VARIANTS: [Variant; 9] = [
    variant("Unit", 0, 0),
    variant("Circle", 2, 1),
    variant("Pair", 3, 2),
    variant("None", 5, 0),
    variant("Some", 5, 1),
    variant("Ok", 6, 1),
    variant("Err", 7, 1),
    variant("End", 0, 0),
    variant("Link", 8, 1),
];
const T: Table = Table {
    descs: &DESCS,
    slots: &SLOTS,
    fields: &FIELDS,
    variants: &VARIANTS,
    names: NAMES,
};

struct NoCaps;
impl Caps for NoCaps {
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }

    fn call(&mut self, _: &str, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Err("no capabilities".into())
    }
}

/// The interpreter's value as runtime objects of descriptor `d`; the slot owns its reference.
fn build(d: u32, v: &Value) -> u64 {
    match (T.desc(d), v) {
        (Desc::Unit, Value::Unit) => 0,
        (Desc::Int, Value::Int(n)) => *n as u64,
        (Desc::Bool, Value::Bool(b)) => *b as u64,
        (Desc::Str, Value::Str(s)) => to_slot(obj::new_bytes(desc::STR, s.as_bytes())),
        (Desc::Bytes, Value::Bytes(b)) => to_slot(obj::new_bytes(desc::BYTES, b)),
        (Desc::List { elem }, Value::List(vs)) => {
            to_slot(obj::new_list(d, vs.iter().map(|v| build(elem, v)).collect()))
        }
        (Desc::Map { key, value }, Value::Map(m)) => {
            let mut out = Map::new();
            for (k, v) in m.iter() {
                let k = match k {
                    lang::Key::Int(n) => Key::Int(*n),
                    lang::Key::Str(s) => Key::Str(s.as_bytes().into()),
                };
                assert_eq!(key, if matches!(k, Key::Int(_)) { desc::INT } else { desc::STR });
                out.insert(k, build(value, v));
            }
            to_slot(obj::new_map(d, out))
        }
        (Desc::Record { slots, .. }, Value::Struct(_, vs)) => fill(obj::alloc(&T, d, 0), T.slots(slots), vs),
        (Desc::Enum { variants, .. }, Value::Enum(_, i, vs)) => {
            let slots = T.slots(T.variant(variants, *i as u32).slots);
            fill(obj::alloc(&T, d, *i as u32), slots, vs)
        }
        (d, v) => panic!("{d:?} does not fit {v:?}"),
    }
}

fn fill(p: *mut Header, slots: &[u32], vs: &[Value]) -> u64 {
    assert_eq!(slots.len(), vs.len());
    for (i, (&d, v)) in slots.iter().zip(vs).enumerate() {
        // SAFETY: `p` is a fresh struct or variant with `slots.len()` slots.
        unsafe { *obj::slot(&T, p, i) = build(d, v) };
    }
    to_slot(p)
}

/// The text of a `Str`, then frees it.
fn take(p: *mut Header) -> String {
    // SAFETY: `p` is a live `Str` owned here.
    let s = String::from_utf8(unsafe { obj::bytes(p) }.to_vec()).unwrap();
    // SAFETY: as above.
    unsafe { obj::dec(&T, p) };
    s
}

/// Runs `files`' `main` and checks its display, returning it.
fn check(files: &[(&str, &str)], d: u32) -> String {
    let _l = lock();
    let before = obj::live();
    let p = check_files(files, &CapTable::default()).unwrap_or_else(|e| panic!("{e:?}"));
    let v = run(&p, &mut NoCaps).unwrap();
    let want = p.display(&v);
    let slot = build(d, &v);
    // SAFETY: `slot` was built as a value of `d`.
    let got = take(unsafe { display(&T, d, slot) });
    assert_eq!(got, want);
    if T.is_ptr(d) {
        // SAFETY: `slot` owns a reference.
        unsafe { obj::dec(&T, from_slot(slot)) };
    }
    assert_eq!(obj::live(), before);
    want
}

fn main_of(ret: &str, body: &str) -> String {
    alloc::format!("fn main() -> {ret} {{ {body} }}")
}

fn case(ret: &str, body: &str, d: u32) -> String {
    check(&[("app", &main_of(ret, body))], d)
}

#[test]
fn scalars() {
    assert_eq!(case("Int", "-42", desc::INT), "-42");
    assert_eq!(case("Bool", "true", desc::BOOL), "true");
    assert_eq!(check(&[("app", "fn main() {}")], desc::UNIT), "()");
    assert_eq!(case("Bytes", "\"abc\".to_bytes()", desc::BYTES), "<3 bytes>");
    assert_eq!(case("Bytes", "\"\".to_bytes()", desc::BYTES), "<0 bytes>");
}

#[test]
fn top_level_str_is_bare() {
    assert_eq!(case("Str", "\"a \\\"b\\\"\\n\"", desc::STR), "a \"b\"\n");
    assert_eq!(case("Str", "\"\"", desc::STR), "");
}

#[test]
fn nested_strings_are_quoted_and_escaped() {
    let got = case(
        "List<Str>",
        r#"["plain", "q\"uote", "back\\slash", "new\nline", "tab\t", "nul\0", "héllo 世", ""]"#,
        LIST_STR,
    );
    assert!(
        got.starts_with(r#"["plain", "q\"uote", "back\\slash", "new\nline", "tab\t", "#),
        "{got}"
    );
}

#[test]
fn lists_and_maps() {
    assert_eq!(case("List<Int>", "[1, 2, 3]", LIST_INT), "[1, 2, 3]");
    assert_eq!(case("List<Int>", "[]", LIST_INT), "[]");
    assert_eq!(
        case("List<List<Int>>", "[[1], [], [2, 3]]", LIST_LIST),
        "[[1], [], [2, 3]]"
    );
    assert_eq!(case("List<Bytes>", "[\"ab\".to_bytes()]", LIST_BYTES), "[<2 bytes>]");
    let m = "let mut m: Map<Str, Int> = Map::new(); m.insert(\"b\", 1); m.insert(\"a\\\"\", 2); m";
    assert_eq!(case("Map<Str, Int>", m, MAP_STR_INT), "{\"a\\\"\": 2, \"b\": 1}");
    assert_eq!(case("Map<Str, Int>", "Map::new()", MAP_STR_INT), "{}");
    let m = "let mut m: Map<Int, Str> = Map::new(); m.insert(2, \"x\"); m.insert(-1, \"y\\n\"); m";
    assert_eq!(case("Map<Int, Str>", m, MAP_INT_STR), "{-1: \"y\\n\", 2: \"x\"}");
}

#[test]
fn structs_and_enums() {
    const MOD: &str = "pub struct Pt { x: Int, y: Str }";
    let run = |ret: &str, body: &str, d| {
        let app = alloc::format!("mod web; fn main() -> {ret} {{ {body} }}");
        check(&[("app", &app), ("web", MOD)], d)
    };
    assert_eq!(
        run("web::Pt", "web::Pt { x: 1, y: \"a\\\"\" }", PT),
        "web::Pt { x: 1, y: \"a\\\"\" }"
    );
    let got = run(
        "List<web::Pt>",
        "[web::Pt { x: 1, y: \"a\" }, web::Pt { x: 2, y: \"b\" }]",
        LIST_PT,
    );
    assert_eq!(got, "[web::Pt { x: 1, y: \"a\" }, web::Pt { x: 2, y: \"b\" }]");

    let shape = "enum Shape { Unit, Circle(Int), Pair(Int, Str) } ";
    let s = |body| {
        check(
            &[("app", &alloc::format!("{shape}fn main() -> Shape {{ {body} }}"))],
            SHAPE,
        )
    };
    assert_eq!(s("Shape::Unit"), "Shape::Unit");
    assert_eq!(s("Shape::Circle(2)"), "Shape::Circle(2)");
    assert_eq!(s("Shape::Pair(1, \"x\")"), "Shape::Pair(1, \"x\")");
    assert_eq!(check(&[("app", "struct E {} fn main() -> E { E {} }")], EMPTY), "E");
}

#[test]
fn option_and_result() {
    assert_eq!(case("Option<Int>", "Some(3)", OPT_INT), "Some(3)");
    assert_eq!(case("Option<Int>", "None", OPT_INT), "None");
    assert_eq!(case("Result<Option<Int>, Str>", "Ok(Some(1))", RES), "Ok(Some(1))");
    assert_eq!(case("Result<Option<Int>, Str>", "Ok(None)", RES), "Ok(None)");
    assert_eq!(
        case("Result<Option<Int>, Str>", "Err(\"bad \\\"x\\\"\")", RES),
        "Err(\"bad \\\"x\\\"\")"
    );
}

/// `main`'s outcome: an `Err` fails and shows its payload alone, a top-level `Str` bare.
#[test]
fn outcome_of_main() {
    let _l = lock();
    let before = obj::live();
    for (ret, src, d, want) in [
        (
            "Result<Option<Int>, Str>",
            "Err(\"bad \\\"x\\\"\")",
            RES,
            ("bad \"x\"", true),
        ),
        ("Result<Option<Int>, Str>", "Ok(Some(1))", RES, ("Ok(Some(1))", false)),
        ("Option<Int>", "Some(2)", OPT_INT, ("Some(2)", false)),
    ] {
        let prog = check_files(&[("app", &main_of(ret, src))], &CapTable::default()).unwrap();
        let slot = build(d, &run(&prog, &mut NoCaps).unwrap());
        // SAFETY: `slot` was built as a value of `d`.
        let (s, failed) = unsafe { crate::display::outcome(&T, d, slot) };
        assert_eq!((take(s).as_str(), failed), want);
        // SAFETY: `slot` owns a reference.
        unsafe { obj::dec(&T, from_slot(slot)) };
    }
    assert_eq!(obj::live(), before);
}

/// Shapes the interpreter's recursion cannot reach, so the expected strings are built here.
#[test]
fn deep_values_do_not_overflow_the_stack() {
    let _l = lock();
    let depth = if cfg!(miri) { 1_000 } else { 1_000_000 };
    let before = obj::live();

    let mut chain = obj::alloc(&T, CHAIN, 0);
    let mut nest = obj::new_list(DEEP, Vec::new());
    for _ in 0..depth {
        let q = obj::alloc(&T, CHAIN, 1);
        // SAFETY: `q` is a fresh `Link`, one slot; it takes `chain`'s reference.
        unsafe { *obj::slot(&T, q, 0) = to_slot(chain) };
        chain = q;
        nest = obj::new_list(DEEP, alloc::vec![to_slot(nest)]);
    }

    // SAFETY: `chain` and `nest` are live values of their descriptors.
    let (c, n) = unsafe {
        (
            take(display(&T, CHAIN, to_slot(chain))),
            take(display(&T, DEEP, to_slot(nest))),
        )
    };
    assert_eq!(c, "Chain::Link(".repeat(depth) + "Chain::End" + &")".repeat(depth));
    assert_eq!(n, "[".repeat(depth + 1) + &"]".repeat(depth + 1));
    // SAFETY: each is live with one reference.
    unsafe {
        obj::dec(&T, chain);
        obj::dec(&T, nest);
    }
    assert_eq!(obj::live(), before);
}
