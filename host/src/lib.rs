//! Runs the guest component under Wasmtime. `dir` is in memory until H1; `http` imports fail. Every import is counted.
use std::collections::BTreeMap;
use std::time::Duration;
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::Result;
use wasmtime::{Engine, Store};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ world: "guest", path: "../caps.wit" });

const GUEST: &[u8] = include_bytes!("../../target/guest/wasm32-wasip2/release/guest.wasm");

/// An in-memory directory in the resource table.
struct MemDir {
    name: String,
    files: BTreeMap<String, Vec<u8>>,
}

/// Store state. `calls` counts every capability import the guest made.
pub struct State {
    pub calls: u32,
    /// Directories the guest has dropped, kept for inspection.
    closed: Vec<MemDir>,
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
            closed: Vec::new(),
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

impl State {
    /// The directory behind a guest handle; `None` for a handle that is not a live `MemDir`.
    fn dir(&mut self, h: &Resource<kio::caps::fs::Dir>) -> Option<&mut MemDir> {
        self.table.get_mut(&Resource::<MemDir>::new_borrow(h.rep())).ok()
    }
}

impl kio::caps::fs::HostDir for State {
    fn read(&mut self, d: Resource<kio::caps::fs::Dir>, path: String) -> Result<Vec<u8>, kio::caps::fs::IoErr> {
        use kio::caps::fs::IoErr;
        self.calls += 1;
        let dir = self.dir(&d).ok_or(IoErr::Denied)?;
        dir.files.get(&path).cloned().ok_or(IoErr::NotFound)
    }

    fn create(
        &mut self,
        d: Resource<kio::caps::fs::Dir>,
        path: String,
        data: Vec<u8>,
    ) -> Result<(), kio::caps::fs::IoErr> {
        self.calls += 1;
        let dir = self.dir(&d).ok_or(kio::caps::fs::IoErr::Denied)?;
        dir.files.insert(path, data);
        Ok(())
    }

    fn drop(&mut self, d: Resource<kio::caps::fs::Dir>) -> Result<()> {
        if let Ok(dir) = self.table.delete(Resource::<MemDir>::new_own(d.rep())) {
            self.closed.push(dir);
        }
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

    /// Check `source` and run `main` with an in-memory directory granted for each `(name, files)`.
    pub fn run(
        &mut self,
        source: &str,
        dirs: Vec<(String, BTreeMap<String, Vec<u8>>)>,
    ) -> Result<std::result::Result<(Report, String), Vec<Diag>>> {
        let mut grants = Vec::new();
        for (name, files) in dirs {
            let h = self.store.data_mut().table.push(MemDir {
                name: name.clone(),
                files,
            })?;
            grants.push(DirGrant {
                name,
                dir: Resource::new_own(h.rep()),
            });
        }
        self.guest.call_run(&mut self.store, source, &grants, &[])
    }

    /// Files of the dropped directory granted as `name`, once a run has finished.
    pub fn files(&self, name: &str) -> Option<&BTreeMap<String, Vec<u8>>> {
        let closed = &self.store.data().closed;
        closed.iter().find(|d| d.name == name).map(|d| &d.files)
    }
}
