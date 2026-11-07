//! Compiled kio: the host checks, lowers and compiles the program, then runs it in the
//! `compiled` world (caps.wit, docs/codegen.md). The interpreter path (`Kio`) is unchanged.

use crate::{config, new_store, set_stack, Limits, State};
use codegen::Site;
use lang::Diag;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Result};

mod bindings {
    wasmtime::component::bindgen!({
        world: "compiled",
        path: "../caps.wit",
        imports: { default: trappable },
        // The capability interfaces are the guest world's, so `State` implements them once.
        with: { "kio:caps/fs": crate::kio::caps::fs, "kio:caps/net": crate::kio::caps::net, "kio:caps/web": crate::kio::caps::web },
    });
}
use bindings::{Compiled as World, CompiledImports, Outcome};

/// A `trap` call: the run stops with the error at `site`.
#[derive(Debug)]
struct Trapped {
    site: u32,
}

impl std::fmt::Display for Trapped {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "kio runtime error at site {}", self.site)
    }
}

impl std::error::Error for Trapped {}

impl CompiledImports for State {
    // C2b formats messages that show `a` and `b`.
    fn trap(&mut self, site: u32, _a: i64, _b: i64) -> Result<()> {
        Err(wasmtime::Error::new(Trapped { site }))
    }
}

/// One compiled program, run once.
pub struct Compiled;

impl Compiled {
    /// Check `source`, compile it and run `main`: its displayed value, the checker's
    /// diagnostics, or a runtime error with the interpreter's span and message.
    pub fn run(source: &str, limits: Limits) -> Result<std::result::Result<Outcome, Vec<Diag>>> {
        let caps = lang::CapTable::v0();
        let program = match lang::check(source, &caps) {
            Ok(p) => p,
            Err(diags) => return Ok(Err(diags)),
        };
        let wasm = codegen::compile(&lang::lower::lower(&program, &caps)).map_err(wasmtime::Error::from_anyhow)?;
        let mut config = config::compile_config();
        set_stack(&mut config, limits.stack);
        let engine = Engine::new(&config)?;
        let component = Component::new(&engine, &wasm.component)?;
        let mut linker = Linker::new(&engine);
        World::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
        let mut store = new_store(&engine, &limits)?;
        let world = World::instantiate(&mut store, &component, &linker)?;
        match world.call_run(&mut store, &[]) {
            Ok(Ok(outcome)) => Ok(Ok(outcome)),
            Ok(Err(())) => wasmtime::bail!("the compiled program refused its grants"),
            Err(e) => match e.downcast_ref::<Trapped>() {
                Some(t) => Ok(Err(vec![runtime_err(&wasm.sites[t.site as usize])])),
                None => Err(e),
            },
        }
    }
}

/// The interpreter's error for site `s`, as the diagnostic `Kio::run` gives.
fn runtime_err(s: &Site) -> Diag {
    Diag {
        span: s.span,
        msg: s.msg.to_string(),
        hint: None,
    }
}
