//! Codegen: an `ir::Module` to a component in the `compiled` world (caps.wit).
//!
//! The program is appended to runtime.wasm, a complete core module (docs/codegen.md,
//! "Pipeline"). Nothing is inserted before what the runtime already has, so no index moves:
//! new types, functions and data go after the runtime's, and the runtime's two placeholder
//! entry functions keep their indexes and get new bodies. wit-component then wraps the one
//! module as a component.

use anyhow::{anyhow, Context, Result};
use lang::ir::{self, BinOp, ExprKind, Module, Ty};
use lang::Span;
use std::collections::HashMap;
use std::fmt::Debug;
use wasm_encoder::reencode::{self, Reencode};
use wasm_encoder::{
    CodeSection, ConstExpr, DataSection, Function, InstructionSink, MemorySection, TypeSection, ValType,
};
use wasmparser::{Parser, Payload};

const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtime.wasm"));
const PAGE: u32 = 1 << 16;

/// A compiled program: the component, and what each `trap` site reports.
pub struct Wasm {
    pub component: Vec<u8>,
    pub sites: Vec<Site>,
}

/// A runtime error the program can raise, by the number generated code passes to `trap`.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub span: Span,
    pub msg: &'static str,
}

/// Compiles `m`. Panics with `unimplemented` on IR that codegen does not handle yet.
pub fn compile(m: &Module) -> Result<Wasm> {
    let rt = Runtime::scan(RUNTIME)?;
    let mut sites = Vec::new();
    let funcs: Vec<(FuncSig, Function)> = m.funcs.iter().map(|f| func(&rt, f, &mut sites)).collect();
    let data_addr = rt.pages * PAGE;
    let main = &m.funcs[m.main.0 as usize];
    let main_index = rt.funcs + m.main.0;
    let mut app = Append {
        rt: &rt,
        funcs,
        entry: entry(rt.export("rt_init"), data_addr, main_index, &main.ret),
        entry_desc: entry_desc(&main.ret),
        data: (data_addr, table(data_addr)),
    };
    let mut module = wasm_encoder::Module::new();
    app.parse_core_module(&mut module, Parser::new(0), RUNTIME)
        .map_err(|e| anyhow!("re-encoding the runtime: {e}"))?;
    let core = module.finish();
    let component = wit_component::ComponentEncoder::default()
        .module(&core)?
        .validate(true)
        .encode()
        .context("wrapping the module as a component")?;
    Ok(Wasm { component, sites })
}

/// What codegen needs to know of runtime.wasm before appending to it.
struct Runtime {
    types: u32,
    /// Function count, imports included: the index of the first appended function.
    funcs: u32,
    imported: u32,
    pages: u32,
    exports: HashMap<String, u32>,
}

impl Runtime {
    fn scan(wasm: &[u8]) -> Result<Runtime> {
        let mut rt = Runtime {
            types: 0,
            funcs: 0,
            imported: 0,
            pages: 0,
            exports: HashMap::new(),
        };
        for payload in Parser::new(0).parse_all(wasm) {
            match payload? {
                Payload::TypeSection(s) => rt.types = s.count(),
                Payload::ImportSection(s) => {
                    for i in s.into_imports() {
                        rt.imported += matches!(i?.ty, wasmparser::TypeRef::Func(_)) as u32;
                    }
                }
                Payload::FunctionSection(s) => rt.funcs = rt.imported + s.count(),
                Payload::MemorySection(s) => {
                    let m = s.into_iter().next().context("the runtime has no memory")??;
                    rt.pages = u32::try_from(m.initial)?;
                }
                Payload::ExportSection(s) => {
                    for e in s {
                        let e = e?;
                        rt.exports.insert(e.name.to_string(), e.index);
                    }
                }
                _ => {}
            }
        }
        Ok(rt)
    }

    fn export(&self, name: &str) -> u32 {
        self.exports[name]
    }
}

/// A Wasm function type.
type FuncSig = (Vec<ValType>, Vec<ValType>);

/// The value type of a kio type, if it has one.
fn val_type(t: &Ty) -> Option<ValType> {
    match t {
        Ty::Int => Some(ValType::I64),
        t => unimplemented!("codegen: type {}", node(t)),
    }
}

/// The variant name of an IR node, for `unimplemented`.
fn node(n: &impl Debug) -> String {
    let s = format!("{n:?}");
    s.split(['(', ' ', '{']).next().unwrap_or_default().to_string()
}

fn func(rt: &Runtime, f: &ir::Func, sites: &mut Vec<Site>) -> (FuncSig, Function) {
    let params = f.locals[..f.params as usize]
        .iter()
        .filter_map(|l| val_type(&l.ty))
        .collect();
    let mut g = FuncGen {
        rt,
        sites,
        temps: Vec::new(),
        first_temp: f.params,
        code: Vec::new(),
    };
    if let Some(s) = f.body.stmts.first() {
        unimplemented!("codegen: statement {}", node(&s.kind));
    }
    match &f.body.tail {
        Some(e) => g.expr(e),
        None => unimplemented!("codegen: a body without a tail"),
    }
    InstructionSink::new(&mut g.code).end();
    let mut body = Function::new_with_locals_types(g.temps);
    body.raw(g.code);
    ((params, val_type(&f.ret).into_iter().collect()), body)
}

struct FuncGen<'a> {
    rt: &'a Runtime,
    sites: &'a mut Vec<Site>,
    /// Hidden locals, after the parameters.
    temps: Vec<ValType>,
    first_temp: u32,
    code: Vec<u8>,
}

impl FuncGen<'_> {
    fn sink(&mut self) -> InstructionSink<'_> {
        InstructionSink::new(&mut self.code)
    }

    fn temp(&mut self, t: ValType) -> u32 {
        self.temps.push(t);
        self.first_temp + self.temps.len() as u32 - 1
    }

    fn site(&mut self, span: Span, msg: &'static str) -> u32 {
        self.sites.push(Site { span, msg });
        self.sites.len() as u32 - 1
    }

    fn expr(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Int(n) => {
                self.sink().i64_const(*n);
            }
            ExprKind::Binary(BinOp::Add, a, b) if e.ty == Ty::Int => {
                self.expr(a);
                self.expr(b);
                self.checked_add(e.span);
            }
            k => unimplemented!("codegen: expression {}", node(k)),
        }
    }

    /// `a + b` from the stack; overflow traps as the interpreter's "Int overflow". It
    /// overflowed when the sum's sign differs from both operands': `(a ^ r) & (b ^ r) < 0`.
    fn checked_add(&mut self, span: Span) {
        let (a, b, r) = (
            self.temp(ValType::I64),
            self.temp(ValType::I64),
            self.temp(ValType::I64),
        );
        let site = self.site(span, "Int overflow");
        let trap = self.rt.export("rt_trap");
        let mut s = self.sink();
        s.local_set(b).local_set(a);
        s.local_get(a).local_get(b).i64_add().local_set(r);
        s.local_get(a).local_get(r).i64_xor();
        s.local_get(b).local_get(r).i64_xor();
        s.i64_and().i64_const(0).i64_lt_s();
        s.if_(wasm_encoder::BlockType::Empty);
        s.i32_const(site as i32)
            .local_get(a)
            .local_get(b)
            .call(trap)
            .unreachable();
        s.end();
        s.local_get(r);
    }
}

/// The body of the runtime's `kio_main`: register the descriptor table, run `main`, return
/// its value as a slot.
fn entry(rt_init: u32, table: u32, main: u32, ret: &Ty) -> Function {
    let mut f = Function::new([]);
    f.instructions().i32_const(table as i32).call(rt_init).call(main);
    match ret {
        Ty::Int => {}
        t => unimplemented!("codegen: main returning {}", node(t)),
    }
    f.instructions().end();
    f
}

/// The body of `kio_main_desc`: the descriptor of main's value.
fn entry_desc(ret: &Ty) -> Function {
    let desc = match ret {
        Ty::Int => 1,
        t => unimplemented!("codegen: main returning {}", node(t)),
    };
    let mut f = Function::new([]);
    f.instructions().i32_const(desc).end();
    f
}

/// The descriptor table as data at `addr`: the runtime's `RawTable` (five address and length
/// pairs), then the descriptors every program has, `Unit`, `Int`, `Bool`, `Str`, `Bytes`,
/// each a `u32` tag padded to 20 bytes (runtime/src/desc.rs).
fn table(addr: u32) -> Vec<u8> {
    let descs = addr + 40;
    let mut raw = vec![descs, 5];
    // The other arrays are empty; an empty slice still needs an aligned address.
    raw.extend([descs, 0].repeat(4));
    let mut data: Vec<u8> = raw.iter().flat_map(|w| w.to_le_bytes()).collect();
    for tag in 0u32..5 {
        data.extend(tag.to_le_bytes());
        data.extend([0; 16]);
    }
    data
}

/// Re-encodes the runtime with the program appended.
struct Append<'a> {
    rt: &'a Runtime,
    funcs: Vec<(FuncSig, Function)>,
    entry: Function,
    entry_desc: Function,
    /// The program's data: its address, at the old end of memory, and its bytes.
    data: (u32, Vec<u8>),
}

impl Append<'_> {
    fn extra_pages(&self) -> u32 {
        (self.data.1.len() as u32).div_ceil(PAGE)
    }
}

type ReResult = std::result::Result<(), reencode::Error<std::convert::Infallible>>;

impl Reencode for Append<'_> {
    type Error = std::convert::Infallible;

    fn parse_type_section(&mut self, types: &mut TypeSection, s: wasmparser::TypeSectionReader<'_>) -> ReResult {
        reencode::utils::parse_type_section(self, types, s)?;
        for ((params, results), _) in &self.funcs {
            types.ty().function(params.clone(), results.clone());
        }
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        funcs: &mut wasm_encoder::FunctionSection,
        s: wasmparser::FunctionSectionReader<'_>,
    ) -> ReResult {
        reencode::utils::parse_function_section(self, funcs, s)?;
        for i in 0..self.funcs.len() as u32 {
            funcs.function(self.rt.types + i);
        }
        Ok(())
    }

    fn parse_memory_section(&mut self, mems: &mut MemorySection, s: wasmparser::MemorySectionReader<'_>) -> ReResult {
        for m in s {
            let mut m = self.memory_type(m?)?;
            m.minimum += u64::from(self.extra_pages());
            mems.memory(m);
        }
        Ok(())
    }

    fn parse_code_section(&mut self, code: &mut CodeSection, s: wasmparser::CodeSectionReader<'_>) -> ReResult {
        let (main, desc) = (self.rt.export("kio_main"), self.rt.export("kio_main_desc"));
        for (i, body) in s.into_iter().enumerate() {
            let body = body?;
            match self.rt.imported + i as u32 {
                f if f == main => code.function(&self.entry),
                f if f == desc => code.function(&self.entry_desc),
                _ => code.raw(body.as_bytes()),
            };
        }
        for (_, f) in &self.funcs {
            code.function(f);
        }
        Ok(())
    }

    fn parse_data_section(&mut self, data: &mut DataSection, s: wasmparser::DataSectionReader<'_>) -> ReResult {
        reencode::utils::parse_data_section(self, data, s)?;
        let (addr, bytes) = &self.data;
        data.active(0, &ConstExpr::i32_const(*addr as i32), bytes.iter().copied());
        Ok(())
    }

    fn data_count(&mut self, count: u32) -> std::result::Result<u32, reencode::Error<Self::Error>> {
        Ok(count + 1)
    }
}

#[cfg(test)]
mod tests;
