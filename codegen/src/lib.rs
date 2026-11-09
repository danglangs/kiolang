//! Codegen: an `ir::Module` to a component in the `compiled` world (caps.wit).
//!
//! The program is appended to runtime.wasm, a complete core module (docs/codegen.md,
//! "Pipeline"). Nothing is inserted before what the runtime already has, so no index moves:
//! new types, functions and data go after the runtime's, and the runtime's two placeholder
//! entry functions keep their indexes and get new bodies. wit-component then wraps the one
//! module as a component.

mod data;

use anyhow::{anyhow, Context, Result};
use data::Statics;
use lang::ir::{self, BinOp, Builtin, ExprKind, Module, Mut, StmtKind, Ty, UnOp};
use lang::Span;
use std::collections::HashMap;
use std::fmt::Debug;
use wasm_encoder::reencode::{self, Reencode};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, Encode, Function, Instruction, InstructionSink, MemorySection,
    TypeSection, ValType,
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
    /// The message; the host replaces `{a}` and `{b}` with `trap`'s arguments.
    pub template: &'static str,
}

impl Site {
    /// The message for a `trap(site, a, b)` here.
    pub fn message(&self, a: i64, b: i64) -> String {
        self.template
            .replace("{a}", &a.to_string())
            .replace("{b}", &b.to_string())
    }
}

/// Compiles `m`. Panics with `unimplemented` on IR that codegen does not handle yet.
pub fn compile(m: &Module) -> Result<Wasm> {
    let rt = Runtime::scan(RUNTIME)?;
    let data_addr = rt.pages * PAGE;
    let mut statics = Statics::new(m, data_addr);
    let mut sites = Vec::new();
    let mut types: Vec<FuncSig> = Vec::new();
    let mut funcs = Vec::new();
    for f in &m.funcs {
        let (sig, body) = func(&rt, &mut statics, f, &mut sites);
        let ty = match types.iter().position(|t| *t == sig) {
            Some(i) => i,
            None => {
                types.push(sig);
                types.len() - 1
            }
        };
        funcs.push((rt.types + ty as u32, body));
    }
    let (data, table) = statics.finish();
    let main = &m.funcs[m.main.0 as usize];
    let main_index = rt.funcs + m.main.0;
    let mut app = Append {
        rt: &rt,
        types,
        funcs,
        entry: entry(rt.export("rt_init"), table, main_index, &main.ret),
        entry_desc: entry_desc(&main.ret),
        data: (data_addr, data),
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

/// The value type of a kio type, if it has one: `()` and `!` have none.
fn val_type(t: &Ty) -> Option<ValType> {
    match t {
        Ty::Int => Some(ValType::I64),
        Ty::Bool => Some(ValType::I32),
        Ty::Str | Ty::Bytes => Some(ValType::I32),
        Ty::Unit | Ty::Never => None,
        t => unimplemented!("codegen: type {}", node(t)),
    }
}

/// Whether a value of `t` is a pointer to a counted object, which every expression yields
/// owned (docs/codegen.md, "Ownership").
fn is_ptr(t: &Ty) -> bool {
    matches!(t, Ty::Str | Ty::Bytes)
}

/// The variant name of an IR node, for `unimplemented`.
fn node(n: &impl Debug) -> String {
    let s = format!("{n:?}");
    s.split(['(', ' ', '{']).next().unwrap_or_default().to_string()
}

fn func(rt: &Runtime, statics: &mut Statics, f: &ir::Func, sites: &mut Vec<Site>) -> (FuncSig, Function) {
    // Each IR local with a value gets a Wasm local: parameters first, as Wasm numbers them,
    // then the others; `()` locals get none. Hidden locals follow.
    let mut params = Vec::new();
    let mut decls = Vec::new();
    let mut locals = Vec::new();
    for (i, l) in f.locals.iter().enumerate() {
        let index = (params.len() + decls.len()) as u32;
        let vt = val_type(&l.ty);
        locals.push(vt.map(|_| index));
        match vt {
            Some(t) if i < f.params as usize => params.push(t),
            Some(t) => decls.push(t),
            None => {}
        }
    }
    // The parameters are owned by the call, as if bound by `let` around the body.
    let owned = (0..f.params as usize)
        .filter(|&i| is_ptr(&f.locals[i].ty))
        .filter_map(|i| locals[i])
        .collect();
    let mut g = FuncGen {
        rt,
        statics,
        sites,
        locals,
        first_decl: params.len() as u32,
        decls,
        scratch: None,
        owned,
        depth: 0,
        loops: Vec::new(),
        code: Vec::new(),
    };
    let ret = val_type(&f.ret);
    g.block(&f.body, ret);
    g.release(0);
    g.sink().end();
    let mut body = Function::new_with_locals_types(g.decls);
    body.raw(g.code);
    ((params, ret.into_iter().collect()), body)
}

struct FuncGen<'a, 'm> {
    rt: &'a Runtime,
    statics: &'a mut Statics<'m>,
    sites: &'a mut Vec<Site>,
    /// The Wasm local of each IR local, `None` for those without a value.
    locals: Vec<Option<u32>>,
    /// Wasm locals after the parameters: the IR's, then hidden ones.
    decls: Vec<ValType>,
    first_decl: u32,
    /// Three `i64` locals an integer check uses, after its operands are on the stack.
    scratch: Option<[u32; 3]>,
    /// The Wasm locals that hold an owned pointer at this point of the code, outermost scope
    /// first: parameters, then each `let` once it has run, and hidden locals holding the
    /// operands evaluated so far. A scope's end releases its part; an exit releases the part
    /// of every scope it leaves.
    owned: Vec<u32>,
    /// Open Wasm blocks, loops and ifs that can hold kio code.
    depth: u32,
    /// For each enclosing kio loop, innermost last: the depth of its `break` and `continue`
    /// targets, and the length of `owned` outside it.
    loops: Vec<(u32, u32, usize)>,
    code: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Open {
    Block,
    Loop,
    If,
}

/// An error message template; `{a}` and `{b}` are `trap`'s arguments.
const OVERFLOW: &str = "Int overflow";
const DIV_ZERO: &str = "division by zero";
const OUT_OF_BOUNDS: &str = "index {a} out of bounds for length {b}";

impl FuncGen<'_, '_> {
    fn sink(&mut self) -> InstructionSink<'_> {
        InstructionSink::new(&mut self.code)
    }

    /// Calls runtime export `name`.
    fn call_rt(&mut self, name: &str) {
        let f = self.rt.export(name);
        self.sink().call(f);
    }

    /// Drops the reference in local `l`.
    fn dec(&mut self, l: u32) {
        self.sink().local_get(l);
        self.call_rt("rt_dec");
    }

    /// Drops the references in `owned[mark..]`, for an exit that leaves those scopes. Code
    /// generation goes on in them, so they stay in `owned`.
    fn release(&mut self, mark: usize) {
        for i in (mark..self.owned.len()).rev() {
            self.dec(self.owned[i]);
        }
    }

    /// Ends the scopes holding `owned[mark..]`: drops their references and zeroes their locals.
    fn end_scope(&mut self, mark: usize) {
        for l in self.owned.split_off(mark).into_iter().rev() {
            self.dec(l);
            self.sink().i32_const(0).local_set(l);
        }
    }

    /// Evaluates `xs` in order, each into a new hidden local, and returns them (`None` for
    /// `()` and `!`). An owned pointer among them is in `owned` from the returned mark on, so
    /// that an exit while a later one runs releases it; the caller truncates `owned` to the
    /// mark once it has used them.
    fn operands(&mut self, xs: &[&ir::Expr]) -> (usize, Vec<Option<u32>>) {
        let mark = self.owned.len();
        let mut temps = Vec::new();
        for x in xs {
            self.expr(x);
            let t = val_type(&x.ty).map(|vt| self.temp(vt));
            if let Some(t) = t {
                self.sink().local_set(t);
                if is_ptr(&x.ty) {
                    self.owned.push(t);
                }
            }
            temps.push(t);
        }
        (mark, temps)
    }

    /// Pushes operand `x` from its hidden local `t`. An operand of type `!` has none, and
    /// the code after it is unreachable: `unreachable` keeps the stack's types valid.
    fn get(&mut self, x: &ir::Expr, t: Option<u32>) {
        match t {
            Some(t) => {
                self.sink().local_get(t);
            }
            None if x.ty == Ty::Never => {
                self.sink().unreachable();
            }
            None => {}
        }
    }

    /// Evaluates `xs`, then runs `f` to use them from their hidden locals without taking
    /// them, then drops the pointers among them: a runtime operation borrows its arguments.
    fn borrowing(&mut self, xs: &[&ir::Expr], f: impl FnOnce(&mut Self, &[Option<u32>])) {
        let (mark, temps) = self.operands(xs);
        f(self, &temps);
        for (x, t) in xs.iter().zip(&temps) {
            if let (true, Some(t)) = (is_ptr(&x.ty), t) {
                self.dec(*t);
            }
        }
        self.owned.truncate(mark);
    }

    /// Calls runtime export `name` on `xs`, which it borrows, with `site` last if it can fail.
    fn rt_op(&mut self, name: &str, xs: &[&ir::Expr], site: Option<u32>) {
        self.borrowing(xs, |g, temps| {
            for (x, t) in xs.iter().zip(temps) {
                g.get(x, *t);
            }
            if let Some(s) = site {
                g.sink().i32_const(s as i32);
            }
            g.call_rt(name);
        });
    }

    fn temp(&mut self, t: ValType) -> u32 {
        self.decls.push(t);
        self.first_decl + self.decls.len() as u32 - 1
    }

    fn scratch(&mut self) -> [u32; 3] {
        if self.scratch.is_none() {
            self.scratch = Some([0; 3].map(|_| self.temp(ValType::I64)));
        }
        self.scratch.unwrap()
    }

    fn site(&mut self, span: Span, template: &'static str) -> u32 {
        self.sites.push(Site { span, template });
        self.sites.len() as u32 - 1
    }

    fn local(&self, id: ir::LocalId) -> Option<u32> {
        self.locals[id.0 as usize]
    }

    /// Opens a Wasm `block`, `loop` or `if` (which takes its condition from the stack); returns
    /// its depth, for `br`.
    fn open(&mut self, kind: Open, bt: BlockType) -> u32 {
        let mut s = self.sink();
        match kind {
            Open::Block => s.block(bt),
            Open::Loop => s.loop_(bt),
            Open::If => s.if_(bt),
        };
        self.depth += 1;
        self.depth
    }

    fn close(&mut self) {
        self.sink().end();
        self.depth -= 1;
    }

    /// A branch to the Wasm block opened at depth `level`.
    fn br(&mut self, level: u32) {
        let rel = self.depth - level;
        self.sink().br(rel);
    }

    /// `b`'s statements and tail, leaving a value of type `want` on the stack. A block without
    /// a tail where a value is wanted diverges.
    fn block(&mut self, b: &ir::Block, want: Option<ValType>) {
        let mark = self.owned.len();
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(e) = &b.tail {
            self.expr(e);
        }
        self.end_scope(mark);
        if b.tail.is_none() && want.is_some() {
            self.sink().unreachable();
        }
    }

    fn stmt(&mut self, s: &ir::Stmt) {
        match &s.kind {
            StmtKind::Let(id, init) => {
                self.expr(init);
                if let Some(l) = self.local(*id) {
                    self.sink().local_set(l);
                    if is_ptr(&init.ty) {
                        self.owned.push(l);
                    }
                }
            }
            StmtKind::Assign(place, op, value) => {
                if let Some(step) = place.steps.first() {
                    unimplemented!("codegen: assignment to {}", node(step));
                }
                let Some(l) = self.local(place.root) else {
                    return self.expr(value);
                };
                match op {
                    // The value first, then the old one dropped.
                    None => {
                        self.expr(value);
                        if is_ptr(&value.ty) {
                            self.dec(l);
                        }
                    }
                    Some(BinOp::Concat) => {
                        // `x + value`, borrowing both, as the runtime does; then the old `x`
                        // and the value are dropped.
                        let v = self.temp(ValType::I32);
                        self.expr(value);
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.call_rt("rt_concat");
                        self.dec(v);
                        self.dec(l);
                    }
                    Some(op) => {
                        // The value comes first, as in the interpreter, then the local's value.
                        self.expr(value);
                        let v = self.scratch()[0];
                        self.sink().local_set(v).local_get(l).local_get(v);
                        self.int_op(*op, s.span);
                    }
                }
                self.sink().local_set(l);
            }
            StmtKind::Return(value) => {
                if let Some(e) = value {
                    self.expr(e);
                }
                self.release(0);
                self.sink().return_();
            }
            StmtKind::While(cond, body) => {
                let brk = self.open(Open::Block, BlockType::Empty);
                let cont = self.open(Open::Loop, BlockType::Empty);
                self.expr(cond);
                self.sink().i32_eqz();
                self.br_if(brk);
                self.loop_body(body, brk, cont);
            }
            StmtKind::Loop(body) => {
                let brk = self.open(Open::Block, BlockType::Empty);
                let cont = self.open(Open::Loop, BlockType::Empty);
                self.loop_body(body, brk, cont);
            }
            StmtKind::ForRange(i, a, b, body) => {
                // Both bounds once, before the first pass; `i` is not `mut`, so it counts.
                let i = self.local(*i).expect("an Int local");
                let end = self.temp(ValType::I64);
                self.expr(a);
                self.expr(b);
                self.sink().local_set(end).local_set(i);
                let brk = self.open(Open::Block, BlockType::Empty);
                let top = self.open(Open::Loop, BlockType::Empty);
                self.sink().local_get(i).local_get(end).i64_ge_s();
                self.br_if(brk);
                let cont = self.open(Open::Block, BlockType::Empty);
                self.loops.push((brk, cont, self.owned.len()));
                self.block(body, None);
                self.loops.pop();
                self.close();
                // Cannot overflow: `i < end`.
                self.sink().local_get(i).i64_const(1).i64_add().local_set(i);
                self.br(top);
                self.close();
                self.close();
            }
            StmtKind::Break => {
                let (brk, _, mark) = *self.loops.last().expect("checked loop");
                self.release(mark);
                self.br(brk);
            }
            StmtKind::Continue => {
                let (_, cont, mark) = *self.loops.last().expect("checked loop");
                self.release(mark);
                self.br(cont);
            }
            StmtKind::Expr(e) => {
                self.expr(e);
                if is_ptr(&e.ty) {
                    self.call_rt("rt_dec");
                } else if val_type(&e.ty).is_some() {
                    self.sink().drop();
                }
            }
            k => unimplemented!("codegen: statement {}", node(k)),
        }
    }

    fn br_if(&mut self, level: u32) {
        let rel = self.depth - level;
        self.sink().br_if(rel);
    }

    /// The body of a `while` or `loop` whose `block` and `loop` are open; closes both.
    fn loop_body(&mut self, body: &ir::Block, brk: u32, cont: u32) {
        self.loops.push((brk, cont, self.owned.len()));
        self.block(body, None);
        self.loops.pop();
        self.br(cont);
        self.close();
        self.close();
    }

    /// Leaves `e`'s value on the stack, if its type has one. An expression of type `!` ends
    /// with `unreachable`, so any context's types check.
    fn expr(&mut self, e: &ir::Expr) {
        match &e.kind {
            ExprKind::Int(n) => {
                self.sink().i64_const(*n);
            }
            ExprKind::Bool(b) => {
                self.sink().i32_const(*b as i32);
            }
            ExprKind::Str(s) => {
                let a = self.statics.str(s);
                self.sink().i32_const(a as i32);
            }
            ExprKind::Unit => {}
            ExprKind::Local(id) => {
                if let Some(l) = self.local(*id) {
                    self.sink().local_get(l);
                    if is_ptr(&e.ty) {
                        // A copy: the local keeps its own reference.
                        self.call_rt("rt_inc");
                        self.sink().local_get(l);
                    }
                }
            }
            ExprKind::Unary(UnOp::Not, x) => {
                self.expr(x);
                self.sink().i32_eqz();
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                self.expr(x);
                let [a, _, _] = self.scratch();
                self.sink().local_tee(a).i64_const(i64::MIN).i64_eq();
                self.trap_if(e.span, OVERFLOW, a, a);
                self.sink().i64_const(0).local_get(a).i64_sub();
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                // The right side runs only when the left does not decide.
                self.expr(a);
                self.open(Open::If, BlockType::Result(ValType::I32));
                if *op == BinOp::And {
                    self.expr(b);
                    self.sink().else_().i32_const(0);
                } else {
                    self.sink().i32_const(1).else_();
                    self.expr(b);
                }
                self.close();
            }
            ExprKind::Binary(BinOp::Concat, a, b) => self.rt_op("rt_concat", &[&**a, &**b], None),
            ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) if is_ptr(&a.ty) => {
                let desc = self.statics.desc(&a.ty);
                self.borrowing(&[&**a, &**b], |g, temps| {
                    g.sink().i32_const(desc as i32);
                    for (x, t) in [a, b].into_iter().zip(temps) {
                        g.get(x, *t);
                        g.sink().i64_extend_i32_u();
                    }
                    g.call_rt("rt_eq");
                    if *op == BinOp::Ne {
                        g.sink().i32_eqz();
                    }
                });
            }
            ExprKind::Binary(op, a, b) => {
                self.expr(a);
                self.expr(b);
                let i = match (op, &a.ty) {
                    (_, Ty::Int) => return self.int_op(*op, e.span),
                    (BinOp::Eq, Ty::Bool) => Instruction::I32Eq,
                    (BinOp::Ne, Ty::Bool) => Instruction::I32Ne,
                    // `()` has no value: equal always.
                    (BinOp::Eq, Ty::Unit) => Instruction::I32Const(1),
                    (BinOp::Ne, Ty::Unit) => Instruction::I32Const(0),
                    (op, t) => unimplemented!("codegen: {op:?} on {}", node(t)),
                };
                i.encode(&mut self.code);
            }
            ExprKind::If(cond, then, els) => {
                let vt = val_type(&e.ty);
                self.expr(cond);
                self.open(Open::If, vt.map_or(BlockType::Empty, BlockType::Result));
                self.block(then, vt);
                if let Some(els) = els {
                    self.sink().else_();
                    self.expr(els);
                }
                self.close();
            }
            ExprKind::Block(b) => self.block(b, val_type(&e.ty)),
            ExprKind::Call(f, args) => {
                // The callee takes the arguments.
                let args: Vec<&ir::Expr> = args.iter().collect();
                let (mark, temps) = self.operands(&args);
                for (x, t) in args.iter().zip(temps) {
                    self.get(x, t);
                }
                self.owned.truncate(mark);
                let index = self.rt.funcs + f.0;
                self.sink().call(index);
            }
            ExprKind::Builtin(b, recv, args) => {
                let (name, error) = match b {
                    Builtin::IntToStr => ("rt_int_to_str", None),
                    Builtin::StrLen => ("rt_str_len", None),
                    Builtin::StrToBytes => ("rt_str_to_bytes", None),
                    Builtin::StrTrim => ("rt_str_trim", None),
                    Builtin::StrContains => ("rt_str_contains", None),
                    Builtin::StrStartsWith => ("rt_str_starts_with", None),
                    Builtin::BytesLen => ("rt_bytes_len", None),
                    Builtin::BytesToStr => ("rt_bytes_to_str", Some("invalid UTF-8")),
                    b => unimplemented!("codegen: builtin {b:?}"),
                };
                let site = error.map(|t| self.site(e.span, t));
                let xs: Vec<&ir::Expr> = std::iter::once(&**recv).chain(args).collect();
                self.rt_op(name, &xs, site);
            }
            ExprKind::Index(base, i) if base.ty == Ty::Bytes => {
                let site = self.site(e.span, OUT_OF_BOUNDS);
                self.rt_op("rt_bytes_get", &[&**base, &**i], Some(site));
            }
            ExprKind::Mutate(place, Mut::BytesPush, args) => {
                if let Some(step) = place.steps.first() {
                    unimplemented!("codegen: mutation through {}", node(step));
                }
                // The byte, then the local made unique and stored back, then the push, whose
                // result (the same object or a larger one) is stored back too.
                let l = self.local(place.root).expect("a Bytes local");
                let site = self.site(e.span, "byte {a} out of range 0..=255");
                let n = self.temp(ValType::I64);
                self.expr(&args[0]);
                self.sink().local_set(n).local_get(l);
                self.call_rt("rt_unique");
                self.sink()
                    .local_set(l)
                    .local_get(l)
                    .local_get(n)
                    .i32_const(site as i32);
                self.call_rt("rt_bytes_push");
                self.sink().local_set(l);
            }
            k => unimplemented!("codegen: expression {}", node(k)),
        }
        if e.ty == Ty::Never {
            self.sink().unreachable();
        }
    }

    /// `a op b` on two `Int` from the stack, with the interpreter's errors at `span`.
    fn int_op(&mut self, op: BinOp, span: Span) {
        let compare = match op {
            BinOp::Lt => Some(Instruction::I64LtS),
            BinOp::Le => Some(Instruction::I64LeS),
            BinOp::Gt => Some(Instruction::I64GtS),
            BinOp::Ge => Some(Instruction::I64GeS),
            BinOp::Eq => Some(Instruction::I64Eq),
            BinOp::Ne => Some(Instruction::I64Ne),
            _ => None,
        };
        if let Some(i) = compare {
            i.encode(&mut self.code);
            return;
        }
        let [a, b, r] = self.scratch();
        self.sink().local_set(b).local_set(a);
        match op {
            BinOp::Add | BinOp::Sub => {
                // Overflow when the result's sign differs from both operands' (`+`), or when
                // the operands' signs differ and the result's differs from a's (`-`).
                let mut s = self.sink();
                s.local_get(a).local_get(b);
                if op == BinOp::Add {
                    s.i64_add().local_set(r);
                    s.local_get(a)
                        .local_get(r)
                        .i64_xor()
                        .local_get(b)
                        .local_get(r)
                        .i64_xor();
                } else {
                    s.i64_sub().local_set(r);
                    s.local_get(a)
                        .local_get(b)
                        .i64_xor()
                        .local_get(a)
                        .local_get(r)
                        .i64_xor();
                }
                s.i64_and().i64_const(0).i64_lt_s();
                self.trap_if(span, OVERFLOW, a, b);
            }
            BinOp::Mul => {
                // Wasm has no overflow flag or 128-bit product: overflow when a != 0 and
                // r / a != b, except that a = -1, b = MIN overflows, and that division traps.
                let mut s = self.sink();
                s.local_get(a).local_get(b).i64_mul().local_set(r);
                s.local_get(a).i64_const(0).i64_ne();
                s.if_(BlockType::Result(ValType::I32));
                s.local_get(a).i64_const(-1).i64_eq();
                s.if_(BlockType::Result(ValType::I32));
                s.local_get(b).i64_const(i64::MIN).i64_eq();
                s.else_();
                s.local_get(r).local_get(a).i64_div_s().local_get(b).i64_ne();
                s.end();
                s.else_().i32_const(0).end();
                self.trap_if(span, OVERFLOW, a, b);
            }
            BinOp::Div | BinOp::Rem => {
                // Wasm's `div_s` traps on 0 and on MIN / -1, and `rem_s` gives 0 for MIN % -1:
                // the interpreter's checks come first, in its order.
                self.sink().local_get(b).i64_eqz();
                self.trap_if(span, DIV_ZERO, a, b);
                let mut s = self.sink();
                s.local_get(a).i64_const(i64::MIN).i64_eq();
                s.local_get(b).i64_const(-1).i64_eq().i32_and();
                self.trap_if(span, OVERFLOW, a, b);
                let mut s = self.sink();
                s.local_get(a).local_get(b);
                if op == BinOp::Div {
                    s.i64_div_s().local_set(r);
                } else {
                    s.i64_rem_s().local_set(r);
                }
            }
            op => unimplemented!("codegen: {op:?} on Int"),
        }
        self.sink().local_get(r);
    }

    /// Traps at a new site when the `i32` on the stack is true, passing locals `a` and `b`.
    fn trap_if(&mut self, span: Span, template: &'static str, a: u32, b: u32) {
        let site = self.site(span, template);
        let trap = self.rt.export("rt_trap");
        let mut s = self.sink();
        s.if_(BlockType::Empty);
        s.i32_const(site as i32)
            .local_get(a)
            .local_get(b)
            .call(trap)
            .unreachable();
        s.end();
    }
}

/// The body of the runtime's `kio_main`: register the descriptor table, run `main`, return
/// its value as a slot.
fn entry(rt_init: u32, table: u32, main: u32, ret: &Ty) -> Function {
    let mut f = Function::new([]);
    f.instructions().i32_const(table as i32).call(rt_init).call(main);
    match ret {
        Ty::Int => {}
        Ty::Bool | Ty::Str | Ty::Bytes => {
            f.instructions().i64_extend_i32_u();
        }
        Ty::Unit => {
            f.instructions().i64_const(0);
        }
        t => unimplemented!("codegen: main returning {}", node(t)),
    }
    f.instructions().end();
    f
}

/// The body of `kio_main_desc`: the descriptor of main's value.
fn entry_desc(ret: &Ty) -> Function {
    let desc = match ret {
        Ty::Unit => 0,
        Ty::Int => 1,
        Ty::Bool => 2,
        Ty::Str => 3,
        Ty::Bytes => 4,
        t => unimplemented!("codegen: main returning {}", node(t)),
    };
    let mut f = Function::new([]);
    f.instructions().i32_const(desc).end();
    f
}

/// Re-encodes the runtime with the program appended.
struct Append<'a> {
    rt: &'a Runtime,
    /// The program's function types, each once, and each function's type index and body.
    types: Vec<FuncSig>,
    funcs: Vec<(u32, Function)>,
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
        for (params, results) in &self.types {
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
        for (ty, _) in &self.funcs {
            funcs.function(*ty);
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
