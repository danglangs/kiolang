//! Random well-typed kio programs, for differential testing of the compiler against the
//! interpreter (docs/compiler-todo.md, C6a).
//!
//! Every program terminates by construction: each `while` and `loop` counts a fresh counter
//! that nothing else assigns to a bound under 50, each `for` spans fewer than 50 values, and a
//! helper recurses only on its first parameter, which nothing assigns or shadows, as `n - 1`
//! after returning when `n <= 0`, and which every outside call passes as `e % 6`. A helper calls
//! only earlier helpers and itself, and calls and loops are generated only while an estimate of
//! the work stays within `BUDGET`. Runtime errors (overflow, division by zero) are allowed.

/// What programs may contain. Int and Bool expressions with every operator, `let`, `let mut`,
/// assignment, `+=`, `-=`, `if`/`else` and `return` are always on.
pub struct Features {
    /// `while`, `loop` and `for` over a range, with `break` and `continue`.
    pub loops: bool,
    /// Helper functions with parameters, called from `main` and from later helpers.
    pub calls: bool,
    /// Helpers that call themselves on a decreasing parameter.
    pub recursion: bool,
}

impl Default for Features {
    /// Everything the compiler supports today.
    fn default() -> Features {
        Features {
            loops: true,
            calls: true,
            recursion: true,
        }
    }
}

/// The source of a program for `seed`; the same seed and features give the same source.
pub fn program(seed: u64, features: &Features) -> String {
    let mut g = Gen {
        rng: Rng::new(seed),
        features,
        funcs: Vec::new(),
        scopes: Vec::new(),
        fresh: 0,
        mult: 1,
        cost: 0,
        loops: 0,
        nest: 0,
        ret: Ty::Unit,
        ind: 0,
        this: None,
    };
    let mut out = String::new();
    if features.calls {
        for i in 0..g.rng.below(5) {
            out += &g.helper(i as usize);
            out += "\n";
        }
    }
    out += &g.main();
    out
}

/// Work estimate, in statements and expression nodes run, above which no call or loop is added.
const BUDGET: u64 = 20_000;
/// Deepest nesting of loops within one function.
const MAX_LOOPS: u32 = 2;
/// Deepest nesting of blocks within one function.
const MAX_NEST: u32 = 3;
/// Deepest expression tree.
const MAX_DEPTH: u32 = 3;

/// xorshift64*, seeded through splitmix64 so nearby seeds give unrelated streams.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    Int,
    Bool,
    Unit,
}

struct Var {
    name: String,
    ty: Ty,
    mutable: bool,
    /// A loop counter or a recursion parameter: never assigned or shadowed.
    fixed: bool,
}

struct Func {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
    recursive: bool,
    /// Estimated work of one call.
    cost: u64,
}

struct Gen<'a> {
    rng: Rng,
    features: &'a Features,
    /// Helpers generated so far: the ones the current function may call.
    funcs: Vec<Func>,
    scopes: Vec<Vec<Var>>,
    fresh: u32,
    /// How many times the current statement runs per call: the product of enclosing loop bounds.
    mult: u64,
    /// Estimated work of the function being generated.
    cost: u64,
    loops: u32,
    nest: u32,
    ret: Ty,
    ind: usize,
    /// The recursive helper being generated: its name and parameters.
    this: Option<(String, Vec<Ty>)>,
}

impl Gen<'_> {
    fn name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}{}", self.fresh)
    }

    fn pad(&self) -> String {
        "    ".repeat(self.ind)
    }

    fn declare(&mut self, name: String, ty: Ty, mutable: bool, fixed: bool) {
        let var = Var {
            name,
            ty,
            mutable,
            fixed,
        };
        self.scopes.last_mut().unwrap().push(var);
    }

    /// The visible variables (innermost binding of each name) matching `keep`.
    fn visible(&self, keep: impl Fn(&Var) -> bool) -> Vec<String> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for v in self.scopes.iter().rev().flat_map(|s| s.iter().rev()) {
            if !seen.contains(&&v.name) {
                seen.push(&v.name);
                if keep(v) {
                    out.push(v.name.clone());
                }
            }
        }
        out
    }

    fn ty(&mut self) -> Ty {
        if self.rng.chance(65) {
            Ty::Int
        } else {
            Ty::Bool
        }
    }

    fn ty_name(t: Ty) -> &'static str {
        match t {
            Ty::Int => "Int",
            Ty::Bool => "Bool",
            Ty::Unit => "()",
        }
    }

    fn helper(&mut self, i: usize) -> String {
        let ret = *self.rng.pick(&[Ty::Int, Ty::Int, Ty::Bool, Ty::Unit]);
        let recursive = self.features.recursion && ret != Ty::Unit && self.rng.chance(35);
        let mut params: Vec<Ty> = (0..self.rng.below(4)).map(|_| self.ty()).collect();
        if recursive {
            params.insert(0, Ty::Int);
        }
        let name = format!("f{i}");
        let names: Vec<String> = (0..params.len()).map(|j| format!("p{j}")).collect();
        self.start(ret);
        for (j, (n, t)) in names.iter().zip(&params).enumerate() {
            self.declare(n.clone(), *t, false, recursive && j == 0);
        }
        let sig: Vec<String> = names
            .iter()
            .zip(&params)
            .map(|(n, t)| format!("{n}: {}", Self::ty_name(*t)))
            .collect();
        let arrow = match ret {
            Ty::Unit => String::new(),
            t => format!(" -> {}", Self::ty_name(t)),
        };
        let mut out = format!("fn {name}({}){arrow} {{\n", sig.join(", "));
        self.ind = 1;
        if recursive {
            let base = self.expr(ret, MAX_DEPTH);
            out += &format!("    if p0 <= 0 {{\n        return {base};\n    }}\n");
            self.this = Some((name.clone(), params.clone()));
        }
        out += &self.body();
        self.this = None;
        out += "}\n";
        // The recursion runs at most 6 deep: outside calls pass `e % 6`.
        let cost = if recursive { self.cost * 6 } else { self.cost };
        self.funcs.push(Func {
            name,
            params,
            ret,
            recursive,
            cost,
        });
        out
    }

    fn main(&mut self) -> String {
        let ret = *self.rng.pick(&[Ty::Int, Ty::Int, Ty::Int, Ty::Bool, Ty::Unit]);
        self.start(ret);
        let arrow = match ret {
            Ty::Unit => String::new(),
            t => format!(" -> {}", Self::ty_name(t)),
        };
        self.ind = 1;
        format!("fn main(){arrow} {{\n{}}}\n", self.body())
    }

    fn start(&mut self, ret: Ty) {
        self.scopes = vec![Vec::new()];
        self.ret = ret;
        self.mult = 1;
        self.cost = 0;
        self.loops = 0;
        self.nest = 0;
    }

    /// A function body's statements and tail, at indent 1, in the function's scope.
    fn body(&mut self) -> String {
        let mut out = String::new();
        for _ in 0..2 + self.rng.below(5) {
            out += &self.stmt();
        }
        let pad = self.pad();
        if self.ret == Ty::Unit {
            return out;
        }
        if self.this.is_some() {
            let tail = self.recursive_tail();
            return out + &format!("{pad}{tail}\n");
        }
        if self.rng.chance(10) {
            let e = self.expr(self.ret, MAX_DEPTH);
            return out + &format!("{pad}return {e};\n");
        }
        let e = self.expr(self.ret, MAX_DEPTH);
        out + &format!("{pad}{e}\n")
    }

    /// The tail of a recursive helper: one call to itself on `p0 - 1`.
    fn recursive_tail(&mut self) -> String {
        let (name, params) = self.this.clone().unwrap();
        let mut args = vec!["(p0 - 1)".to_string()];
        for t in &params[1..] {
            args.push(self.expr(*t, 1));
        }
        let call = format!("{name}({})", args.join(", "));
        let e = self.expr(self.ret, 1);
        match (self.ret, self.rng.below(4)) {
            (Ty::Int, 0) => call,
            (Ty::Int, 1) => format!("({call} + {e})"),
            (Ty::Int, 2) => format!("({e} - {call})"),
            (Ty::Int, _) => format!("({call} * {e})"),
            (_, 0) => format!("(!{call})"),
            (_, 1) => format!("({call} && {e})"),
            (_, 2) => format!("({e} || {call})"),
            (_, _) => format!("({call} == {e})"),
        }
    }

    /// A `{ .. }` block of up to `n` statements, then a tail of type `ret` unless it is `()`.
    fn block(&mut self, n: u64, ret: Ty) -> String {
        self.scopes.push(Vec::new());
        self.ind += 1;
        self.nest += 1;
        let mut out = "{\n".to_string();
        for _ in 0..self.rng.below(n + 1) {
            out += &self.stmt();
        }
        let pad = self.pad();
        if ret != Ty::Unit {
            let e = self.expr(ret, MAX_DEPTH - 1);
            out += &format!("{pad}{e}\n");
        } else if self.loops > 0 && self.rng.chance(8) {
            out += &format!("{pad}{};\n", self.rng.pick(&["break", "continue"]));
        }
        self.nest -= 1;
        self.ind -= 1;
        self.scopes.pop();
        out + &self.pad() + "}"
    }

    fn stmt(&mut self) -> String {
        self.cost += self.mult;
        let pad = self.pad();
        let deep = self.nest >= MAX_NEST;
        loop {
            match self.rng.below(100) {
                0..=29 => return self.let_stmt(),
                30..=49 => {
                    if let Some(s) = self.assign() {
                        return s;
                    }
                }
                50..=61 if !deep => {
                    let c = self.expr(Ty::Bool, 2);
                    let mut s = format!("{pad}if {c} {}", self.block(3, Ty::Unit));
                    if self.rng.chance(50) {
                        s += &format!(" else {}", self.block(3, Ty::Unit));
                    }
                    return s + "\n";
                }
                62..=79 if !deep => {
                    if let Some(s) = self.loop_stmt() {
                        return s;
                    }
                }
                80..=86 => {
                    let t = *self.rng.pick(&[Ty::Int, Ty::Bool, Ty::Unit]);
                    if let Some(c) = self.call(t) {
                        return format!("{pad}{c};\n");
                    }
                }
                87..=93 if self.loops > 0 => {
                    let c = self.expr(Ty::Bool, 2);
                    let k = self.rng.pick(&["break", "continue"]);
                    return format!("{pad}if {c} {{\n{pad}    {k};\n{pad}}}\n");
                }
                94..=99 => {
                    let c = self.expr(Ty::Bool, 2);
                    let r = match self.ret {
                        Ty::Unit => "return;".to_string(),
                        t => format!("return {};", self.expr(t, 2)),
                    };
                    return format!("{pad}if {c} {{\n{pad}    {r}\n{pad}}}\n");
                }
                _ => {}
            }
        }
    }

    fn let_stmt(&mut self) -> String {
        let ty = self.ty();
        let init = self.expr(ty, MAX_DEPTH);
        let shadow = self.visible(|v| !v.fixed);
        let name = if !shadow.is_empty() && self.rng.chance(15) {
            self.rng.pick(&shadow).clone()
        } else {
            self.name("v")
        };
        let mutable = self.rng.chance(50);
        let annot = if self.rng.chance(30) {
            format!(": {}", Self::ty_name(ty))
        } else {
            String::new()
        };
        let m = if mutable { "mut " } else { "" };
        let s = format!("{}let {m}{name}{annot} = {init};\n", self.pad());
        self.declare(name, ty, mutable, false);
        s
    }

    fn assign(&mut self) -> Option<String> {
        let names = self.visible(|v| v.mutable && !v.fixed);
        if names.is_empty() {
            return None;
        }
        let name = self.rng.pick(&names).clone();
        let ty = self.var_ty(&name);
        let op = match ty {
            Ty::Int => *self.rng.pick(&["=", "+=", "-="]),
            _ => "=",
        };
        let e = self.expr(ty, MAX_DEPTH);
        Some(format!("{}{name} {op} {e};\n", self.pad()))
    }

    fn var_ty(&self, name: &str) -> Ty {
        let mut vars = self.scopes.iter().rev().flat_map(|s| s.iter().rev());
        vars.find(|v| v.name == name).unwrap().ty
    }

    fn loop_stmt(&mut self) -> Option<String> {
        if !self.features.loops || self.loops >= MAX_LOOPS || self.cost > BUDGET / 2 {
            return None;
        }
        let pad = self.pad();
        let bound = 1 + self.rng.below(49);
        let kind = self.rng.below(3);
        let c = self.name(if kind == 2 { "i" } else { "c" });
        let runs = if kind == 2 { 49 } else { bound };
        // The head is outside the loop: a `break` in it would not be this loop's.
        let head = match kind {
            0 => {
                self.declare(c.clone(), Ty::Int, true, true);
                let extra = match self.rng.chance(30) {
                    true => format!(" && {}", self.expr(Ty::Bool, 1)),
                    false => String::new(),
                };
                format!("{pad}let mut {c} = 0;\n{pad}while {c} < {bound}{extra}")
            }
            1 => {
                self.declare(c.clone(), Ty::Int, true, true);
                format!("{pad}let mut {c} = 0;\n{pad}loop")
            }
            _ => {
                // From `lo` (at least -9) to `hi` (at most 40): at most 49 runs.
                let lo = match self.rng.chance(50) {
                    true => self.rng.below(10).to_string(),
                    false => format!("({} % 10)", self.expr(Ty::Int, 1)),
                };
                format!("{pad}for {c} in {lo}..{}", self.rng.below(41))
            }
        };
        self.mult *= runs;
        self.loops += 1;
        let body = match kind {
            0 => self.counted_block(&format!("{c} += 1;\n")),
            1 => self.counted_block(&format!(
                "{c} += 1;\n{pad}    if {c} > {bound} {{\n{pad}        break;\n{pad}    }}\n"
            )),
            _ => {
                self.scopes.push(vec![]);
                self.declare(c, Ty::Int, false, false);
                let body = self.block(3, Ty::Unit);
                self.scopes.pop();
                body
            }
        };
        self.loops -= 1;
        self.mult /= runs;
        Some(format!("{head} {body}\n"))
    }

    /// A loop body whose first statement, `step`, counts the loop's counter.
    fn counted_block(&mut self, step: &str) -> String {
        let b = self.block(3, Ty::Unit);
        format!("{{\n{}    {step}{}", self.pad(), &b[2..])
    }

    /// A call of a helper returning `ret`, if one fits the budget.
    fn call(&mut self, ret: Ty) -> Option<String> {
        let room = BUDGET.saturating_sub(self.cost);
        let fits: Vec<usize> = (0..self.funcs.len())
            .filter(|&i| self.funcs[i].ret == ret && self.funcs[i].cost * self.mult <= room)
            .collect();
        if fits.is_empty() {
            return None;
        }
        let f = *self.rng.pick(&fits);
        self.cost += self.funcs[f].cost * self.mult;
        let params = self.funcs[f].params.clone();
        let mut args: Vec<String> = params.iter().map(|t| self.expr(*t, 1)).collect();
        if self.funcs[f].recursive {
            args[0] = format!("({} % 6)", args[0]);
        }
        Some(format!("{}({})", self.funcs[f].name, args.join(", ")))
    }

    fn expr(&mut self, ty: Ty, depth: u32) -> String {
        self.cost += self.mult;
        match ty {
            Ty::Int => self.int(depth),
            Ty::Bool => self.bool(depth),
            Ty::Unit => "()".into(),
        }
    }

    fn leaf(&mut self, ty: Ty) -> String {
        let vars = self.visible(|v| v.ty == ty);
        if !vars.is_empty() && self.rng.chance(60) {
            return self.rng.pick(&vars).clone();
        }
        match ty {
            Ty::Bool => self.rng.pick(&["true", "false"]).to_string(),
            _ => self.literal(),
        }
    }

    fn literal(&mut self) -> String {
        match self.rng.below(10) {
            0..=6 => self.rng.below(10).to_string(),
            7 => self.rng.below(100).to_string(),
            8 => format!("(-{})", self.rng.below(10)),
            _ => {
                let big = ["9223372036854775807", "4611686018427387904", "3037000500", "4294967296"];
                self.rng.pick(&big).to_string()
            }
        }
    }

    fn int(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.chance(25) {
            return self.leaf(Ty::Int);
        }
        let d = depth - 1;
        loop {
            match self.rng.below(10) {
                0..=5 => {
                    let op = *self.rng.pick(&["+", "-", "*", "/", "%"]);
                    let a = self.int(d);
                    // Mostly a nonzero divisor, so most programs run past their divisions.
                    let b = if matches!(op, "/" | "%") && self.rng.chance(75) {
                        (1 + self.rng.below(9)).to_string()
                    } else {
                        self.int(d)
                    };
                    return format!("({a} {op} {b})");
                }
                6 => return format!("(-{})", self.int(d)),
                7 => return self.if_expr(Ty::Int, d),
                _ => {
                    if let Some(c) = self.call(Ty::Int) {
                        return c;
                    }
                }
            }
        }
    }

    fn bool(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.chance(25) {
            return self.leaf(Ty::Bool);
        }
        let d = depth - 1;
        loop {
            match self.rng.below(10) {
                0..=3 => {
                    let op = *self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
                    return format!("({} {op} {})", self.int(d), self.int(d));
                }
                4 | 5 => {
                    let op = *self.rng.pick(&["&&", "||", "==", "!="]);
                    return format!("({} {op} {})", self.bool(d), self.bool(d));
                }
                6 => return format!("(!{})", self.bool(d)),
                7 => return self.if_expr(Ty::Bool, d),
                _ => {
                    if let Some(c) = self.call(Ty::Bool) {
                        return c;
                    }
                }
            }
        }
    }

    /// `if c { .. } else { .. }` as a value; branches hold statements when nesting allows.
    fn if_expr(&mut self, ty: Ty, depth: u32) -> String {
        let c = self.bool(depth);
        if self.nest >= MAX_NEST {
            let (a, b) = (self.expr(ty, depth), self.expr(ty, depth));
            return format!("(if {c} {{ {a} }} else {{ {b} }})");
        }
        let then = self.block(1, ty);
        let els = self.block(1, ty);
        format!("(if {c} {then} else {els})")
    }
}
