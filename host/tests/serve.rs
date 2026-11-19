use kio_host::{open_dir, Limits, Request, Server};
use std::time::Instant;

fn get(target: &str) -> Request {
    Request {
        method: "GET".into(),
        target: target.into(),
        header_names: vec!["host".into()],
        header_values: vec!["localhost".into()],
        body: vec![],
    }
}

#[test]
fn server_is_send_and_sync() {
    fn is<T: Send + Sync>() {}
    is::<Server>();
}

#[test]
fn thousand_requests_get_the_right_body() {
    let read = |p: &str| std::fs::read_to_string(format!("../examples/serve/{p}")).unwrap();
    let files = vec![
        ("app.kio".to_string(), read("app.kio")),
        ("web.kio".to_string(), read("web.kio")),
    ];
    let server = Server::new(files, Limits::web()).unwrap().unwrap();
    assert!(server.shown_report().contains("main"));
    let public = tempfile::tempdir().unwrap();
    let mut times = vec![];
    for _ in 0..1000 {
        let dirs = vec![("public".to_string(), open_dir(public.path()).unwrap())];
        let start = Instant::now();
        let r = server.handle(get("/hello/kio"), dirs).unwrap();
        times.push(start.elapsed());
        assert_eq!(r.status, 200);
        assert_eq!(r.body, br#"{"hello": "kio"}"#);
    }
    times.sort();
    // docs/perf.md records the release median: `cargo test --release --test serve -- --nocapture`.
    println!("Server::handle median {:?}, p99 {:?}", times[500], times[990]);
}

#[test]
fn out_of_fuel_is_a_500_and_the_next_request_works() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        if req.target() == "/loop" { while true {} }
        res.send(200, [], [], "ok".to_bytes())
    }"#;
    let limits = Limits {
        fuel: 10_000_000,
        ..Limits::web()
    };
    let server = Server::new(vec![("main".into(), src.into())], limits).unwrap().unwrap();
    let r = server.handle(get("/loop"), vec![]).unwrap();
    assert_eq!((r.status, r.body.len()), (500, 0));
    let r = server.handle(get("/"), vec![]).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"ok"[..]));
}

#[test]
fn no_response_is_a_500() {
    let src = "fn main(req: Incoming, res: Outgoing) -> Int { 0 }";
    let server = Server::new(vec![("main".into(), src.into())], Limits::web())
        .unwrap()
        .unwrap();
    assert_eq!(server.handle(get("/"), vec![]).unwrap().status, 500);
}

#[test]
fn type_error_returns_the_diags() {
    let src = "fn main(req: Incoming, res: Outgoing) -> Int { \"x\" }";
    let Err(diags) = Server::new(vec![("main".into(), src.into())], Limits::web()).unwrap() else {
        panic!("expected diags");
    };
    assert!(!diags.is_empty());
}

#[test]
fn program_hash_is_sha256_of_names_and_sources() {
    let f = |n: &str, s: &str| (n.to_string(), s.to_string());
    let h = kio_host::program_hash(&[f("a", "x"), f("b", "y")]);
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(h, kio_host::program_hash(&[f("a", "x"), f("b", "y")]));
    assert_ne!(h, kio_host::program_hash(&[f("a", "x"), f("b", "z")]));
    assert_ne!(
        h,
        kio_host::program_hash(&[f("a", "xb"), f("", "y")]),
        "file boundaries count"
    );
}

mod upstream;

/// H19: every request resolves the granted host for itself, once, however many calls it makes:
/// no request connects to an address another request's lookup returned.
#[test]
fn http_dns_is_pinned_per_request() {
    let up = upstream::start();
    let src = r#"fn main(req: Incoming, res: Outgoing, up: Http) -> Result<(), WebErr> {
        let a = match up.get("/a") { Ok(b) => b.to_str(), Err(_) => "failed " };
        let b = match up.get("/b") { Ok(b) => b.to_str(), Err(_) => "failed " };
        res.send(200, [], [], (a + b).to_bytes())
    }"#;
    let mut origin = kio_host::Origin::parse(&up.grant.replace("127.0.0.1", "localhost")).unwrap();
    origin.allow_private_addrs();
    let mut server = Server::new(vec![("main".into(), src.into())], Limits::web())
        .unwrap()
        .unwrap();
    server.grant_https(vec![("up".into(), origin.clone())]);
    for n in 1..=3 {
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"GET /a GET /b "[..]));
        assert_eq!(origin.lookups(), n);
    }
}

/// C17: `--memory` holds under the pool as before: a request that asks for more fails with a
/// 500, and the next request, in a reused slot, works.
#[test]
fn a_request_over_memory_is_a_500_and_the_next_request_works() {
    let src = r#"fn main(req: Incoming, res: Outgoing) -> Result<(), WebErr> {
        let mut s = "ok";
        if req.target() == "/big" { while s.len() < 16000000 { s = s + s; } }
        res.send(200, [], [], s.slice(0, 2).to_bytes())
    }"#;
    let limits = Limits {
        memory: 8 << 20,
        ..Limits::web()
    };
    let server = Server::new(vec![("main".into(), src.into())], limits).unwrap().unwrap();
    for _ in 0..3 {
        let r = server.handle(get("/big"), vec![]).unwrap();
        assert_eq!((r.status, r.body.len()), (500, 0));
        let r = server.handle(get("/"), vec![]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"ok"[..]));
    }
}

/// C17: a fresh instance in a reused pool slot sees nothing an earlier one wrote. Each round,
/// four instances at once grow their memory and write a marker into every word of it; then four
/// new instances, in the same slots (their memories' addresses are the writers'), check the
/// memory is back to its initial size, grow it as the writers did and read every byte: the data
/// image or zero. So both the kept initial pages (reset by `memset` on Linux) and the grown ones
/// (by `madvise`) are read. The engine is the one `Server` builds.
#[test]
fn a_reused_slot_shows_nothing_an_earlier_instance_wrote() {
    use wasm_encoder::BlockType::Empty;
    use wasm_encoder::Instruction as I;
    use wasm_encoder::{
        CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function, FunctionSection, MemArg,
        MemorySection, MemoryType, Module as Enc, TypeSection, ValType,
    };
    use wasmtime::{Linker, Module, Store};
    const PAGES: u64 = 20;
    const DATA: i32 = 1 << 20;
    let arg = |align| MemArg {
        offset: 0,
        align,
        memory_index: 0,
    };
    // fill(marker): grow by 16 pages, then store `marker` at every 8 bytes of memory.
    #[rustfmt::skip]
    let fill = [
        I::I32Const(16), I::MemoryGrow(0), I::Drop,
        I::Loop(Empty),
        I::LocalGet(1), I::LocalGet(0), I::I64Store(arg(3)),
        I::LocalGet(1), I::I32Const(8), I::I32Add, I::LocalTee(1),
        I::MemorySize(0), I::I32Const(16), I::I32Shl, I::I32LtU, I::BrIf(0),
        I::End,
        I::End,
    ];
    // scan(): -1 if memory is not its initial size; else grow by 16 pages and return 1 + the
    // first byte that is neither the data image (bytes 1..=16 at DATA) nor zero; 0 if none.
    #[rustfmt::skip]
    let scan = [
        I::MemorySize(0), I::I32Const(PAGES as i32), I::I32Ne,
        I::If(Empty), I::I32Const(-1), I::Return, I::End,
        I::I32Const(16), I::MemoryGrow(0), I::Drop,
        I::Loop(Empty),
        // memory[i] != (if i - DATA < 16 unsigned { i - DATA + 1 } else { 0 })
        I::LocalGet(0), I::I32Load8U(arg(0)),
        I::LocalGet(0), I::I32Const(DATA), I::I32Sub, I::I32Const(1), I::I32Add,
        I::I32Const(0),
        I::LocalGet(0), I::I32Const(DATA), I::I32Sub, I::I32Const(16), I::I32LtU,
        I::Select,
        I::I32Ne,
        I::If(Empty), I::LocalGet(0), I::I32Const(1), I::I32Add, I::Return, I::End,
        I::LocalGet(0), I::I32Const(1), I::I32Add, I::LocalTee(0),
        I::MemorySize(0), I::I32Const(16), I::I32Shl, I::I32LtU, I::BrIf(0),
        I::End,
        I::I32Const(0),
        I::End,
    ];
    let mut m = Enc::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], []);
    types.ty().function([], [ValType::I32]);
    m.section(&types);
    m.section(FunctionSection::new().function(0).function(1));
    m.section(MemorySection::new().memory(MemoryType {
        minimum: PAGES,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    }));
    m.section(
        ExportSection::new()
            .export("fill", ExportKind::Func, 0)
            .export("scan", ExportKind::Func, 1)
            .export("memory", ExportKind::Memory, 0),
    );
    let mut code = CodeSection::new();
    for body in [&fill[..], &scan[..]] {
        let mut f = Function::new([(1, ValType::I32)]);
        for i in body {
            f.instruction(i);
        }
        code.function(&f);
    }
    m.section(&code);
    m.section(DataSection::new().active(0, &ConstExpr::i32_const(DATA), 1..=16u8));
    let wasm = m.finish();

    let engine = Server::pool_engine_for_test(&Limits::web(), (PAGES << 16) as usize).unwrap();
    let module = Module::new(&engine, &wasm).unwrap();
    let pre = Linker::<()>::new(&engine).instantiate_pre(&module).unwrap();
    let start = || {
        let mut store = Store::new(&engine, ());
        store.set_fuel(u64::MAX).unwrap();
        store.set_epoch_deadline(1 << 40);
        let instance = pre.instantiate(&mut store).unwrap();
        (store, instance)
    };
    let slot = |store: &mut Store<()>, instance: &wasmtime::Instance| {
        let memory = instance.get_memory(&mut *store, "memory").unwrap();
        memory.data_ptr(&*store) as usize
    };
    for round in 0..3 {
        let mut written = vec![];
        let mut slots = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let (mut store, instance) = start();
            slots.insert(slot(&mut store, &instance));
            let fill = instance.get_typed_func::<i64, ()>(&mut store, "fill").unwrap();
            fill.call(&mut store, 0x6b69_6f6b_696f_6b69).unwrap();
            written.push(store);
        }
        drop(written);
        let mut fresh = vec![];
        let mut reused = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let (mut store, instance) = start();
            reused.insert(slot(&mut store, &instance));
            let scan = instance.get_typed_func::<(), i32>(&mut store, "scan").unwrap();
            assert_eq!(
                scan.call(&mut store, ()).unwrap(),
                0,
                "round {round}: 1 + the first stale byte"
            );
            fresh.push(store);
        }
        assert_eq!(reused, slots, "round {round}: the scans ran in the writers' slots");
    }
}
