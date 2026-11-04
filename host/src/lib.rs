//! Runs the guest component under Wasmtime. G2: every capability import fails and is counted.
use std::time::Duration;
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ world: "guest", path: "../caps.wit" });

const GUEST: &[u8] = include_bytes!("../../target/guest/wasm32-wasip2/release/guest.wasm");

/// Store state. `calls` counts every capability import the guest made.
pub struct State {
    pub calls: u32,
    wasi: WasiCtx,
    table: ResourceTable,
}

/// A clock that never moves, so a run cannot observe time.
struct Frozen;

impl HostWallClock for Frozen {
    fn resolution(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

impl HostMonotonicClock for Frozen {
    fn resolution(&self) -> u64 {
        1
    }
    fn now(&self) -> u64 {
        0
    }
}

impl State {
    /// WASI is linked only because Rust's std for wasm32-wasip2 imports it. It grants nothing:
    /// no preopens, env, args, network or stdio, and frozen clocks and fixed seeds so every run of
    /// the same program behaves the same. kio code reaches the host only through caps.wit.
    fn new() -> Self {
        let wasi = WasiCtx::builder()
            .wall_clock(Frozen)
            .monotonic_clock(Frozen)
            .insecure_random_seed(0)
            .build();
        State {
            calls: 0,
            wasi,
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl kio::caps::fs::Host for State {}
impl kio::caps::net::Host for State {}

impl kio::caps::fs::HostDir for State {
    fn read(&mut self, _: Resource<kio::caps::fs::Dir>, _: String) -> Result<Vec<u8>, kio::caps::fs::IoErr> {
        self.calls += 1;
        Err(kio::caps::fs::IoErr::Denied)
    }

    fn create(&mut self, _: Resource<kio::caps::fs::Dir>, _: String, _: Vec<u8>) -> Result<(), kio::caps::fs::IoErr> {
        self.calls += 1;
        Err(kio::caps::fs::IoErr::Denied)
    }

    fn drop(&mut self, _: Resource<kio::caps::fs::Dir>) -> Result<()> {
        Ok(())
    }
}

impl kio::caps::net::HostHttp for State {
    fn get(&mut self, _: Resource<kio::caps::net::Http>, _: String) -> Result<Vec<u8>, kio::caps::net::HttpErr> {
        self.calls += 1;
        Err(kio::caps::net::HttpErr::Denied)
    }

    fn drop(&mut self, _: Resource<kio::caps::net::Http>) -> Result<()> {
        Ok(())
    }
}

/// A fresh instance of the embedded guest.
pub struct Kio {
    store: Store<State>,
    guest: Guest,
}

impl Kio {
    pub fn new() -> Result<Self> {
        let engine = Engine::default();
        let component = Component::new(&engine, GUEST)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Guest::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |s| s)?;
        let mut store = Store::new(&engine, State::new());
        let guest = Guest::instantiate(&mut store, &component, &linker)?;
        Ok(Kio { store, guest })
    }

    /// Parse and check `source`: the report lines, or the diagnostics.
    pub fn check(&mut self, source: &str) -> Result<std::result::Result<Report, Vec<Diag>>> {
        self.guest.call_check(&mut self.store, source)
    }

    /// Capability imports the guest has called so far.
    pub fn import_calls(&self) -> u32 {
        self.store.data().calls
    }
}
