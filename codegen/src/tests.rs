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

#[test]
#[should_panic(expected = "codegen: type Cap")]
fn unsupported_ir_names_the_node() {
    let _ = compile(&module("fn main(input: Dir) {}"));
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
    use lang::ir::{EnumId, StructId};
    use runtime::desc::{Desc, RawTable};
    use runtime::display::display;
    use runtime::obj::{self, to_slot, Header, PINNED};

    let m = module(
        r#"
struct Tree { name: Str, kids: List<Tree> }
enum Shape { Dot, Line(Int, Tree) }
fn main() -> Int {
    let t = Tree { name: "a", kids: [] };
    let s = Shape::Line(1, t);
    let o: Option<Int> = Some(5);
    let r: Result<Map<Str, Int>, Bytes> = Ok(Map::new());
    match "b" { "a" => 1, _ => 0 }
}"#,
    );
    // Not page-aligned, so an address relative to the data would show.
    const BASE: u32 = 24;
    let mut s = Statics::new(&m, BASE);
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
    // The program's six other types, all added from the module: the lookups above added none.
    assert_eq!(t.descs.len(), 12);
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
    assert_eq!(obj::live(), before);
    // SAFETY: a pinned static is never freed.
    assert_eq!(unsafe { (*at(a)).rc }, PINNED);
}
