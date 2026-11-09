use super::*;

fn module(src: &str) -> Module {
    let caps = lang::CapTable::v0();
    let p = lang::check(src, &caps).unwrap();
    lang::lower::lower(&p, &caps)
}

#[test]
fn the_component_validates() {
    let wasm = compile(&module("fn main() -> Int { 40 + 2 }")).unwrap();
    wasmparser::Validator::new().validate_all(&wasm.component).unwrap();
    assert_eq!(
        wasm.sites,
        [Site {
            span: Span {
                start: 19,
                end: 25,
                file: 0
            },
            template: "Int overflow"
        }]
    );
}

/// Every caps.wit method, called from a program: the runtime has a wrapper for each
/// (runtime/src/caps.rs), and the component validates, so each wrapper's signature is the one
/// codegen calls. The grant kinds `rt_grant` numbers are the `CapTable`'s order.
#[test]
fn every_capability_method_compiles() {
    let caps = lang::CapTable::v0();
    let names: Vec<&str> = caps.caps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["Dir", "Http", "Incoming", "Outgoing", "Kv", "WsMessage", "WsSend"],
        "runtime/src/entry.rs `run` numbers caps.wit's `grant` cases in this order"
    );
    let mut calls = 0;
    let mut body = String::new();
    for c in &caps.caps {
        for (method, params, _) in &c.methods {
            let args: Vec<&str> = params
                .iter()
                .map(|t| match t {
                    lang::check::Ty::Str => "\"a\"",
                    lang::check::Ty::Bytes => "\"a\".to_bytes()",
                    lang::check::Ty::Int => "1",
                    lang::check::Ty::List(_) => "[\"a\"]",
                    lang::check::Ty::Option(_) => "Some(\"a\".to_bytes())",
                    t => panic!("a test argument of type {t:?}"),
                })
                .collect();
            body += &format!(
                "    let r{calls} = {}.{method}({});\n",
                c.name.to_lowercase(),
                args.join(", ")
            );
            calls += 1;
        }
    }
    let params: Vec<String> = names.iter().map(|n| format!("{}: {n}", n.to_lowercase())).collect();
    let src = format!("fn main({}) {{\n{body}}}", params.join(", "));
    let wasm = compile(&module(&src)).unwrap();
    wasmparser::Validator::new().validate_all(&wasm.component).unwrap();
    assert_eq!(calls, 24);
}

#[test]
fn a_site_formats_its_template() {
    let span = Span {
        start: 0,
        end: 1,
        file: 0,
    };
    let s = Site {
        span,
        template: "index {a} out of bounds for length {b}",
    };
    assert_eq!(s.message(-1, 3, 0), "index -1 out of bounds for length 3");
}

#[test]
fn the_runtime_exports_every_operation() {
    let rt = Runtime::scan(RUNTIME).unwrap();
    let ops = [
        "rt_list_new",
        "rt_map_new",
        "rt_eq",
        "rt_concat",
        "rt_int_to_str",
        "rt_str_len",
        "rt_str_to_bytes",
        "rt_str_split",
        "rt_str_lines",
        "rt_str_trim",
        "rt_str_contains",
        "rt_str_starts_with",
        "rt_str_parse_int",
        "rt_bytes_len",
        "rt_bytes_to_str",
        "rt_bytes_get",
        "rt_list_len",
        "rt_list_get",
        "rt_map_len",
        "rt_map_keys",
        "rt_map_get",
        "rt_option_unwrap_or",
        "rt_option_is_some",
        "rt_option_is_none",
        "rt_result_unwrap_or",
        "rt_result_is_ok",
        "rt_result_is_err",
        "rt_list_set",
        "rt_list_push",
        "rt_bytes_push",
        "rt_map_insert",
        "rt_map_remove",
        "rt_live",
    ];
    let missing: Vec<_> = ops.iter().filter(|op| !rt.exports.contains_key(**op)).collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn the_descriptor_table_reads_back_through_the_runtime() {
    use lang::ir::{EnumId, FuncId, StructId};
    use runtime::desc::{Desc, RawTable};
    use runtime::display::display;
    use runtime::obj::{self, to_slot, Header, PINNED};

    let m = module(
        r#"
struct Tree { name: Str, kids: List<Tree> }
enum Shape { Dot, Line(Int, Tree) }
fn id(x: Int) -> Int { x }
fn main() -> Int {
    let k = "a";
    let f = |x: Int| k;
    let t = Tree { name: "a", kids: [] };
    let s = Shape::Line(1, t);
    let o: Option<Int> = Some(5);
    let r: Result<Map<Str, Int>, Bytes> = Ok(Map::new());
    match "b" { "a" => 1, _ => 0 }
}"#,
    );
    // Not page-aligned, so an address relative to the data would show.
    const BASE: u32 = 24;
    const TABLE: u32 = 46;
    let mut s = Statics::new(&m, BASE, TABLE);
    let tree = s.desc(&Ty::Struct(StructId(0)));
    let shape = s.desc(&Ty::Enum(EnumId(m.enums.len() as u32 - 1)));
    let list_tree = s.desc(&Ty::List(Box::new(Ty::Struct(StructId(0)))));
    let opt = s.desc(&Ty::Option(Box::new(Ty::Int)));
    let map_ty = Ty::Map(Box::new(Ty::Str), Box::new(Ty::Int));
    let res = s.desc(&Ty::Result(Box::new(map_ty.clone()), Box::new(Ty::Bytes)));
    let map = s.desc(&map_ty);
    let a = s.str("a");
    assert_eq!(
        (a, s.str("b")),
        (BASE, BASE + 24),
        "literals are interned and 8-aligned"
    );
    let none = s.pinned(&Ty::Option(Box::new(Ty::Int)), 0);
    assert_eq!(
        (none, s.pinned(&Ty::Option(Box::new(Ty::Int)), 0)),
        (BASE + 48, BASE + 48)
    );
    let dot = s.pinned(&Ty::Enum(EnumId(m.enums.len() as u32 - 1)), 0);
    let func = |name: &str| FuncId(m.funcs.iter().position(|f| f.name.contains(name)).unwrap() as u32);
    let fn_ty = s.desc(&Ty::Fn(vec![Ty::Int], Box::new(Ty::Str)));
    let item = s.pinned_fn(func("id"), None);
    assert_eq!(s.pinned_fn(func("id"), None), item, "one pinned object per function");
    let (closure, elem) = s.func(func("closure"), Some(1));
    assert_eq!(s.elems(), [(func("id"), 0), (func("closure"), 1)]);
    assert_eq!(elem, TABLE + 1);
    let (data, table) = s.finish();

    // Memory as Wasm sees it: address `x` is byte `x` of `buf`, which is 8-aligned.
    let mut buf = vec![0u64; (BASE as usize + data.len()).div_ceil(8)];
    // SAFETY: `buf` has room for `data` after BASE bytes.
    unsafe {
        let p = buf.as_mut_ptr().cast::<u8>().add(BASE as usize);
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    }
    let base = buf.as_ptr().expose_provenance();
    let at = |addr: u32| std::ptr::with_exposed_provenance_mut::<Header>(base + addr as usize);
    // SAFETY: `finish` laid out a `RawTable` and its arrays at `table`; `buf` outlives `t`.
    let t = unsafe { RawTable::read(base, table) };

    assert_eq!(
        t.descs[..6],
        [Desc::Unit, Desc::Int, Desc::Bool, Desc::Str, Desc::Bytes, Desc::Float]
    );
    // The program's seven other types, all added from the module: the lookups above added
    // none. Then the two functions used as values.
    assert_eq!(t.descs.len(), 15);
    let Desc::Record { name, slots } = t.desc(tree) else {
        panic!("{:?}", t.desc(tree))
    };
    assert_eq!(t.name(name), b"Tree");
    assert_eq!(t.slots(slots), [3, list_tree]);
    assert_eq!(t.field_name(slots, 0), b"name");
    assert_eq!(t.field_name(slots, 1), b"kids");
    assert_eq!(t.desc(list_tree), Desc::List { elem: tree });
    assert_eq!(t.desc(map), Desc::Map { key: 3, value: 1 });

    // SAFETY: the literal is a pinned `Str` in `buf`.
    unsafe {
        assert_eq!(((*at(a)).rc, (*at(a)).desc), (PINNED, 3));
        assert_eq!(obj::bytes(at(a)), b"a");
    }

    // Values built on the table display as the interpreter shows them, and free completely.
    let before = obj::live();
    let show = |d: u32, p: *mut Header| {
        // SAFETY: `p` is a live object of `d`, owned here.
        unsafe {
            let s = display(&t, d, to_slot(p));
            let text = String::from_utf8(obj::bytes(s).to_vec()).unwrap();
            obj::dec(&t, s);
            obj::dec(&t, p);
            text
        }
    };
    let tr = obj::alloc(&t, tree, 0);
    let sh = obj::alloc(&t, shape, 1);
    // SAFETY: fresh objects with the slots their descriptors give.
    unsafe {
        *obj::slot(&t, tr, 0) = to_slot(at(a));
        *obj::slot(&t, tr, 1) = to_slot(obj::new_list(list_tree, Vec::new()));
        *obj::slot(&t, sh, 0) = 1;
        *obj::slot(&t, sh, 1) = to_slot(tr);
    }
    assert_eq!(show(shape, sh), r#"Shape::Line(1, Tree { name: "a", kids: [] })"#);
    let o = obj::alloc(&t, opt, 1);
    // SAFETY: `Some` has one `Int` slot.
    unsafe { *obj::slot(&t, o, 0) = 5 };
    assert_eq!(show(opt, o), "Some(5)");
    let r = obj::alloc(&t, res, 0);
    // SAFETY: `Ok` has one `Map` slot.
    unsafe { *obj::slot(&t, r, 0) = to_slot(obj::new_map(map, Default::default())) };
    assert_eq!(show(res, r), "Ok({})");
    // A pinned variant: displayed, copied and dropped, never counted or freed.
    // SAFETY: `none` and `dot` are pinned enum objects in `buf`.
    unsafe {
        assert_eq!(
            ((*at(none)).rc, (*at(none)).desc, *obj::field::<u32>(at(none), 8)),
            (PINNED, opt, 0)
        );
        for _ in 0..3 {
            obj::inc(at(none));
            obj::dec(&t, at(none));
            obj::dec(&t, at(dot));
        }
        let s = display(&t, opt, to_slot(at(none)));
        assert_eq!(obj::bytes(s), b"None");
        obj::dec(&t, s);
        let s = display(&t, shape, to_slot(at(dot)));
        assert_eq!(obj::bytes(s), b"Shape::Dot");
        obj::dec(&t, s);
        assert_eq!(((*at(none)).rc, (*at(dot)).rc), (PINNED, PINNED));
    }
    // Function values: a pinned item and a closure owning its capture, each displayed through
    // a `fn` type's descriptor, which names neither.
    let Desc::Fn { name, slots } = t.desc(fn_ty) else {
        panic!("{:?}", t.desc(fn_ty))
    };
    assert_eq!((name.len, slots.len), (0, 0));
    let Desc::Fn { name, slots } = t.desc(closure) else {
        panic!("{:?}", t.desc(closure))
    };
    assert_eq!(
        (t.name(name), t.slots(slots)),
        (b"<closure>".as_slice(), [3].as_slice())
    );
    let c = obj::alloc(&t, closure, elem);
    // SAFETY: `item` is a pinned function object in `buf`; `c` a fresh closure with one `Str` slot.
    unsafe {
        let d = (*at(item)).desc;
        assert_eq!(((*at(item)).rc, *obj::field::<u32>(at(item), 8)), (PINNED, TABLE));
        assert!(matches!(t.desc(d), Desc::Fn { name, .. } if t.name(name) == b"<fn id>"));
        let s = display(&t, fn_ty, to_slot(at(item)));
        assert_eq!(obj::bytes(s), b"<fn id>");
        obj::dec(&t, s);
        assert_eq!(*obj::field::<u32>(c, 8), elem);
        *obj::slot(&t, c, 0) = to_slot(obj::new_bytes(3, b"k"));
    }
    assert_eq!(show(fn_ty, c), "<closure>");
    assert_eq!(obj::live(), before);
    // SAFETY: a pinned static is never freed.
    assert_eq!(unsafe { (*at(a)).rc }, PINNED);
}

/// C8b4: a pointer parameter read only in place is borrowed; one read where a reference is
/// taken (stored, returned, moved, captured, an owned argument) is owned, also through calls
/// (`a` passes `s` to `b`, which passes it back and to `keep`, which stores it) and recursion.
#[test]
fn parameters_read_in_place_are_borrowed() {
    let m = module(
        r#"
enum T { Leaf(Str), Node(List<T>) }
struct P { name: Str }
fn read(s: Str, xs: List<Int>, n: Int, p: P, f: fn(Int) -> Int, o: Option<Str>) -> Int {
    let k = o.unwrap_or("");
    let mut m = n;
    for x in xs { m += x; }
    s.len() + xs[0] + m + p.name.len() + f(1) + k.len() + if s == "a" { 1 } else { 0 }
}
fn opt(o: Option<Int>) -> Option<Int> { let x = o?; Some(x) }
fn stored(s: Str) -> List<Str> { [s] }
fn returned(s: Str) -> Str { s }
fn moved(xs: List<Int>) -> List<Int> { let mut ys = xs; ys.push(1); ys }
fn captured(s: Str) -> Int { let f = |x: Int| s.len() + x; f(1) }
fn owned_arg(s: Str) -> Str { returned(s) }
fn lent_arg(s: Str) -> Int { read(s, [1], 2, P { name: s + "" }, |x: Int| x, None) }
fn size(t: T) -> Int { match t { T::Leaf(s) => s.len(), T::Node(ks) => { let mut n = 0; for k in ks { n += size(k); } n } } }
fn a(s: Str, n: Int) -> Int { if n == 0 { keep(s) } else { b(s, n - 1) } }
fn b(s: Str, n: Int) -> Int { a(s, n) }
fn keep(s: Str) -> Int { stored(s).len() }
fn main() -> Int { 0 }
"#,
    );
    let lent = moves::borrowed(&m, is_ptr);
    let of = |name: &str| lent[m.funcs.iter().position(|f| f.name == name).unwrap()].clone();
    assert_eq!(of("read"), [true, true, false, true, true, true]);
    assert_eq!(of("opt"), [true]);
    for owned in ["stored", "returned", "moved", "captured", "owned_arg", "a", "b", "keep"] {
        assert!(!of(owned)[0], "{owned}");
    }
    assert_eq!(of("lent_arg"), [true]);
    assert_eq!(of("size"), [true]);
    // The closure in `captured` reads its capture in place.
    assert_eq!(of("captured::{closure#0}"), [true, false]);
}
