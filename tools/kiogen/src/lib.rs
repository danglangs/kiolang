//! Random well-typed kio programs, for differential testing of the compiler against the
//! interpreter (docs/compiler-todo.md, C6a).
//!
//! Every program terminates by construction: each `while` and `loop` counts a fresh counter
//! that nothing else assigns to a bound under 50, each `for` spans fewer than 50 values, and a
//! helper recurses only on its first parameter, which nothing assigns or shadows, as `n - 1`
//! after returning when `n <= 0`, and which every outside call passes as `e % 6`. A helper calls
//! only earlier helpers and itself, and calls and loops are generated only while an estimate of
//! the work stays within `BUDGET`. A `for` over a list runs at most `MAX_LEN` passes (or the
//! list's tracked bound, outside other loops), since a `push` is generated only while that
//! bound stays within `MAX_LEN`. Runtime errors (overflow, division by zero, an index out of
//! bounds) are allowed.
//!
//! With `adts`, every program declares `struct S { a: Int, b: Bool }` and
//! `enum K { Z, One(Int), Two(Int, Option<Int>) }`. A `match` lists some of a fixed sequence of
//! arms, each more specific than or disjoint from the later ones, then `_`, which always has a
//! value left to match (`K::Two(_, None)`, or `None`), so every arm is reachable.
//!
//! With `strings`, every `Str` has a bound on its length (`Var::smax`), kept under `MAX_STR`:
//! an append inside a loop adds its parts' bound times the loop's passes, and only a mutable
//! `Str` declared as an accumulator (`Var::acc`) may be appended to there. Inside a loop no
//! `Str` value that is stored reads an accumulator, whose bound grows as the loop runs; only
//! `len`, `==`, `contains`, `split(..).len()` and `match` read one. A parameter is at most
//! `PARAM_MAX` bytes and a helper's result at most `RET_MAX`.

/// What programs may contain. Int and Bool expressions with every operator, `let`, `let mut`,
/// assignment, `+=`, `-=`, `if`/`else` and `return` are always on.
pub struct Features {
    /// `while`, `loop` and `for` over a range, with `break` and `continue`.
    pub loops: bool,
    /// Helper functions with parameters, called from `main` and from later helpers.
    pub calls: bool,
    /// Helpers that call themselves on a decreasing parameter.
    pub recursion: bool,
    /// `List<Int>` locals: literals, copies, indexing (mostly in range), `len`, `push`,
    /// element assignment and `+=`, `==`, `for x in xs`, `contains`, `sort` and `pop`.
    pub lists: bool,
    /// The struct `S`, the enum `K` and `Option<Int>`: locals, construction, copies, field
    /// reads and writes, `==`, `unwrap_or`, `is_some`, `match` with nested patterns and a
    /// catch-all, `if let`, and helpers returning `Option<Int>` that use `?`.
    pub adts: bool,
    /// `Float` locals, parameters and results: literals (NaN, infinities, `-0.0`, the
    /// extremes, halves), every operator, `+=` and `-=`, comparisons, `to_str`, `to_int`,
    /// `to_float`, `parse_float`, `abs`, `floor`, `ceil`, `round`, `sqrt` and the `is_` tests.
    pub floats: bool,
    /// `map`, `filter`, `any` and `all` on `List<Int>` values, each with a closure applying one
    /// operator to the element and a literal or a captured `Int` local.
    pub closures: bool,
    /// `Str` locals, parameters and results: literals, `+`, `+=`, `s = s + a + b` (the
    /// compiler's in-place append) and `s = s + s`, `to_str`, `trim`, `len`, `==`, `contains`,
    /// `match` on literals, and `split` and `lines` into `List<Str>` locals, indexed, counted,
    /// searched and iterated with `for`.
    pub strings: bool,
}

impl Default for Features {
    /// Everything the compiler supports today.
    fn default() -> Features {
        Features {
            loops: true,
            calls: true,
            recursion: true,
            lists: true,
            adts: true,
            floats: true,
            closures: true,
            strings: true,
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
        ret_bound: MAX_STR,
    };
    let mut out = String::new();
    if features.adts {
        out += "struct S { a: Int, b: Bool }\nenum K { Z, One(Int), Two(Int, Option<Int>) }\n";
    }
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
/// Longest a list can grow: a `push` is generated only while the list's bound stays within it.
const MAX_LEN: u64 = 100;
/// Longest a `Str` can grow, in bytes, and the most a parameter or a helper's result holds.
const MAX_STR: u64 = 2000;
const PARAM_MAX: u64 = 64;
const RET_MAX: u64 = 256;
/// `Str` literals as written, and their length in bytes: empty, short, with separators, a
/// two-byte character, and long ones sharing a prefix (the runtime compares 8 bytes at a time).
const STR_LITS: &[(&str, u64)] = &[
    ("\"\"", 0),
    ("\"a\"", 1),
    ("\"b\"", 1),
    ("\"ab\"", 2),
    ("\"a,b\"", 3),
    ("\"x y z\"", 5),
    ("\"a\\nb\\n\"", 4),
    ("\"\u{e9}\"", 2),
    ("\"abcdefgh\"", 8),
    ("\"abcdefghi\"", 9),
    ("\"prefix-shared-key-1\"", 19),
    ("\"prefix-shared-key-2\"", 19),
];

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
    /// `List<Int>`; only locals have it.
    List,
    Float,
    /// The struct `S`, the enum `K` and `Option<Int>`; locals, and `Opt` as a return type.
    S,
    K,
    Opt,
    Str,
    /// `List<Str>`, from `split` or `lines`; only immutable locals have it.
    StrList,
}

struct Var {
    name: String,
    ty: Ty,
    mutable: bool,
    /// A loop counter or a recursion parameter: never assigned or shadowed.
    fixed: bool,
    /// For a list: bounds on its length at any point of the function. Indexes below `min` are
    /// in range; `max` bounds the passes of a `for` over it and grows with each `push`.
    min: u64,
    max: u64,
    /// For a `Str`: a bound on its length; for a `List<Str>`, on each element's.
    smax: u64,
    /// A mutable `Str` that loops may append to, so read inside no loop but as `len`, `==`,
    /// `contains` and the like.
    acc: bool,
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
    /// The longest `Str` the function being generated may return.
    ret_bound: u64,
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
            min: 0,
            max: 0,
            smax: 0,
            acc: false,
        };
        self.scopes.last_mut().unwrap().push(var);
    }

    /// The innermost binding of `name`.
    fn var(&mut self, name: &str) -> &mut Var {
        let mut vars = self.scopes.iter_mut().rev().flat_map(|s| s.iter_mut().rev());
        vars.find(|v| v.name == name).unwrap()
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
        if self.features.floats && self.rng.chance(20) {
            Ty::Float
        } else if self.rng.chance(65) {
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
            Ty::List => "List<Int>",
            Ty::Float => "Float",
            Ty::S => "S",
            Ty::K => "K",
            Ty::Opt => "Option<Int>",
            Ty::Str => "Str",
            Ty::StrList => "List<Str>",
        }
    }

    fn helper(&mut self, i: usize) -> String {
        let mut ret = *self.rng.pick(&[Ty::Int, Ty::Int, Ty::Bool, Ty::Unit]);
        if self.features.adts && self.rng.chance(30) {
            ret = Ty::Opt;
        } else if self.features.floats && self.rng.chance(20) {
            ret = Ty::Float;
        } else if self.features.strings && self.rng.chance(15) {
            ret = Ty::Str;
        }
        let recursive = self.features.recursion && matches!(ret, Ty::Int | Ty::Bool) && self.rng.chance(35);
        let mut params: Vec<Ty> = (0..self.rng.below(4)).map(|_| self.ty()).collect();
        if self.features.strings {
            for p in params.iter_mut() {
                if self.rng.chance(15) {
                    *p = Ty::Str;
                }
            }
        }
        if recursive {
            params.insert(0, Ty::Int);
        }
        let name = format!("f{i}");
        let names: Vec<String> = (0..params.len()).map(|j| format!("p{j}")).collect();
        self.start(ret);
        self.ret_bound = RET_MAX;
        for (j, (n, t)) in names.iter().zip(&params).enumerate() {
            self.declare(n.clone(), *t, false, recursive && j == 0);
            self.var(n).smax = PARAM_MAX;
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
        let mut ret = *self.rng.pick(&[Ty::Int, Ty::Int, Ty::Int, Ty::Bool, Ty::Unit]);
        if self.features.adts && self.rng.chance(10) {
            ret = *self.rng.pick(&[Ty::S, Ty::K, Ty::Opt]);
        } else if self.features.floats && self.rng.chance(15) {
            ret = Ty::Float;
        } else if self.features.strings && self.rng.chance(10) {
            ret = Ty::Str;
        }
        self.start(ret);
        self.ret_bound = MAX_STR;
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
            args.push(self.arg(*t));
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
        if self.features.lists && self.rng.chance(20) {
            if let Some(s) = self.list_stmt() {
                return s;
            }
        }
        if self.features.adts && self.rng.chance(15) {
            if let Some(s) = self.adt_stmt() {
                return s;
            }
        }
        if self.features.strings && self.rng.chance(25) {
            if let Some(s) = self.str_stmt() {
                return s;
            }
        }
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
        if self.features.lists && self.rng.chance(20) {
            return self.let_list();
        }
        let ty = match self.features.adts && self.rng.chance(25) {
            true => *self.rng.pick(&[Ty::S, Ty::K, Ty::Opt]),
            false => self.ty(),
        };
        let init = self.expr(ty, MAX_DEPTH);
        let shadow = self.visible(|v| !v.fixed);
        let name = if !shadow.is_empty() && self.rng.chance(15) {
            self.rng.pick(&shadow).clone()
        } else {
            self.name("v")
        };
        let mutable = self.rng.chance(50);
        let annot = if ty == Ty::Opt || self.rng.chance(30) {
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
        let names = self.visible(|v| v.mutable && !v.fixed && !matches!(v.ty, Ty::List | Ty::Str | Ty::StrList));
        if names.is_empty() {
            return None;
        }
        let name = self.rng.pick(&names).clone();
        let ty = self.var_ty(&name);
        let op = match ty {
            Ty::Int | Ty::Float => *self.rng.pick(&["=", "+=", "-="]),
            _ => "=",
        };
        // The checker does not type a bare `None` from the assigned local.
        let e = match ty {
            Ty::Opt => self.some(MAX_DEPTH),
            _ => self.expr(ty, MAX_DEPTH),
        };
        Some(format!("{}{name} {op} {e};\n", self.pad()))
    }

    fn var_ty(&self, name: &str) -> Ty {
        let mut vars = self.scopes.iter().rev().flat_map(|s| s.iter().rev());
        vars.find(|v| v.name == name).unwrap().ty
    }

    /// A list value: a copy of a visible list or a literal of up to five elements (at least
    /// one unless `empty`, since `[]` needs a type from its context). Returns it with the
    /// bounds on its length.
    fn list_expr(&mut self, empty: bool) -> (String, u64, u64) {
        if self.features.closures && self.rng.chance(10) {
            // Not `[]`, which has no type before `.map`.
            let (a, min, max) = self.list_expr(false);
            if self.rng.chance(50) {
                return (format!("{a}.map({})", self.closure(false)), min, max);
            }
            return (format!("{a}.filter({})", self.closure(true)), 0, max);
        }
        let lists = self.visible(|v| v.ty == Ty::List);
        if !lists.is_empty() && self.rng.chance(30) {
            let name = self.rng.pick(&lists).clone();
            let v = self.var(&name);
            let (min, max) = (v.min, v.max);
            return (name, min, max);
        }
        let n = self.rng.below(6).max(u64::from(!empty));
        let items: Vec<String> = (0..n).map(|_| self.expr(Ty::Int, 1)).collect();
        (format!("[{}]", items.join(", ")), n, n)
    }

    /// A closure on an element `el` for `map` (`Int` to `Int`) or, if `pred`, for `filter`,
    /// `any` and `all`: one operator on `el` and a leaf, which may capture a visible `Int`.
    fn closure(&mut self, pred: bool) -> String {
        let ops: &[&str] = if pred {
            &["<", "<=", ">", ">=", "==", "!="]
        } else {
            &["+", "-", "*", "/", "%"]
        };
        let op = *self.rng.pick(ops);
        format!("|el| el {op} {}", self.leaf(Ty::Int))
    }

    fn let_list(&mut self) -> String {
        let (init, min, max) = self.list_expr(true);
        let name = self.name("xs");
        let mutable = self.rng.chance(70);
        let annot = if init == "[]" || self.rng.chance(30) {
            ": List<Int>"
        } else {
            ""
        };
        let m = if mutable { "mut " } else { "" };
        let s = format!("{}let {m}{name}{annot} = {init};\n", self.pad());
        self.declare(name.clone(), Ty::List, mutable, false);
        let v = self.var(&name);
        (v.min, v.max) = (min, max);
        s
    }

    /// An index into list `name`: mostly a literal known to be in range, else any `Int`.
    fn index(&mut self, name: &str) -> String {
        let min = self.var(name).min;
        if min > 0 && self.rng.chance(85) {
            self.rng.below(min).to_string()
        } else {
            format!("({} % 8)", self.expr(Ty::Int, 0))
        }
    }

    /// `push`, an element's `=`, `+=` or `-=`, the whole list's `=`, `for x in xs`, `sort` or
    /// `pop`.
    fn list_stmt(&mut self) -> Option<String> {
        let pad = self.pad();
        let kind = self.rng.below(8);
        if kind == 4 || kind == 5 {
            return self.for_list();
        }
        let mult = self.mult;
        let mutable: Vec<String> = self
            .visible(|v| v.ty == Ty::List && v.mutable && !v.fixed)
            .into_iter()
            .filter(|n| kind > 1 || self.var(n).max + mult <= MAX_LEN)
            .collect();
        if mutable.is_empty() {
            return None;
        }
        let name = self.rng.pick(&mutable).clone();
        match kind {
            0 | 1 => {
                let e = self.expr(Ty::Int, 2);
                self.var(&name).max += mult;
                Some(format!("{pad}{name}.push({e});\n"))
            }
            2 => {
                let i = self.index(&name);
                let op = *self.rng.pick(&["=", "+=", "-="]);
                let e = self.expr(Ty::Int, 2);
                Some(format!("{pad}{name}[{i}] {op} {e};\n"))
            }
            6 => Some(format!("{pad}{name}.sort();\n")),
            7 => {
                // A pass of an enclosing loop pops again: the list may be shorter by `mult`.
                let v = self.var(&name);
                v.min = v.min.saturating_sub(mult);
                Some(format!("{pad}{name}.pop();\n"))
            }
            _ => {
                let (e, min, max) = self.list_expr(false);
                let v = self.var(&name);
                (v.min, v.max) = (v.min.min(min), v.max.max(max));
                Some(format!("{pad}{name} = {e};\n"))
            }
        }
    }

    /// `for x in xs` over a visible list. Its passes are bounded by the list's `max`, or by
    /// `MAX_LEN` inside another loop, whose later passes may have pushed more.
    fn for_list(&mut self) -> Option<String> {
        let lists = self.visible(|v| v.ty == Ty::List);
        if lists.is_empty()
            || !self.features.loops
            || self.loops >= MAX_LOOPS
            || self.nest >= MAX_NEST
            || self.cost > BUDGET / 2
        {
            return None;
        }
        let name = self.rng.pick(&lists).clone();
        let runs = match self.loops {
            0 => self.var(&name).max.max(1),
            _ => MAX_LEN,
        };
        let pad = self.pad();
        let x = self.name("x");
        self.mult *= runs;
        self.loops += 1;
        self.scopes.push(vec![]);
        self.declare(x.clone(), Ty::Int, false, true);
        let body = self.block(3, Ty::Unit);
        self.scopes.pop();
        self.loops -= 1;
        self.mult /= runs;
        Some(format!("{pad}for {x} in {name} {body}\n"))
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
        let mut args: Vec<String> = params.iter().map(|t| self.arg(*t)).collect();
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
            Ty::Float => self.float(depth),
            Ty::Unit => "()".into(),
            Ty::List | Ty::StrList => unreachable!("lists come from list_expr and str_stmt"),
            Ty::S | Ty::K | Ty::Opt => self.adt(ty, depth),
            Ty::Str => self.str_upto(depth, self.ret_bound),
        }
    }

    /// An argument for a parameter of type `t`.
    fn arg(&mut self, t: Ty) -> String {
        match t {
            Ty::Str => self.str_upto(1, PARAM_MAX),
            t => self.expr(t, 1),
        }
    }

    /// `xs[i]` or `xs.len()` on a visible list, if there is one.
    fn list_int(&mut self) -> Option<String> {
        let lists = self.visible(|v| v.ty == Ty::List);
        if lists.is_empty() {
            return None;
        }
        let name = self.rng.pick(&lists).clone();
        if self.rng.chance(30) {
            return Some(format!("{name}.len()"));
        }
        let i = self.index(&name);
        Some(format!("{name}[{i}]"))
    }

    fn leaf(&mut self, ty: Ty) -> String {
        let vars = self.visible(|v| v.ty == ty);
        if !vars.is_empty() && self.rng.chance(60) {
            return self.rng.pick(&vars).clone();
        }
        match ty {
            Ty::Bool => self.rng.pick(&["true", "false"]).to_string(),
            Ty::Float => self.float_literal(),
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
        if depth > 0 && self.features.lists && self.rng.chance(12) {
            if let Some(e) = self.list_int() {
                return e;
            }
        }
        if depth > 0 && self.features.adts && self.rng.chance(15) {
            return self.adt_int(depth - 1);
        }
        if depth > 0 && self.features.strings && self.rng.chance(8) {
            return self.str_int(depth - 1);
        }
        if depth > 0 && self.features.floats && self.rng.chance(8) {
            let x = self.float(depth - 1);
            return match self.rng.below(2) {
                0 => format!("({x}).to_int().unwrap_or({})", self.int(depth - 1)),
                _ => format!("({x}).to_str().len()"),
            };
        }
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
        let lists = self.features.lists && !self.visible(|v| v.ty == Ty::List).is_empty();
        if lists && self.rng.chance(6) {
            let (a, _, _) = self.list_expr(false);
            let (b, _, _) = self.list_expr(false);
            let op = *self.rng.pick(&["==", "!="]);
            return format!("({a} {op} {b})");
        }
        if lists && self.rng.chance(4) {
            let (a, _, _) = self.list_expr(false);
            let x = self.expr(Ty::Int, 1);
            return format!("{a}.contains({x})");
        }
        if lists && self.features.closures && self.rng.chance(5) {
            let (a, _, _) = self.list_expr(false);
            let m = *self.rng.pick(&["any", "all"]);
            return format!("{a}.{m}({})", self.closure(true));
        }
        if depth > 0 && self.features.adts && self.rng.chance(10) {
            return self.adt_bool(depth - 1);
        }
        if depth > 0 && self.features.strings && self.rng.chance(8) {
            return self.str_bool(depth - 1);
        }
        if depth > 0 && self.features.floats && self.rng.chance(10) {
            let d = depth - 1;
            if self.rng.chance(30) {
                let m = *self.rng.pick(&["is_nan", "is_finite", "is_infinite"]);
                return format!("({}).{m}()", self.float(d));
            }
            let op = *self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
            return format!("({} {op} {})", self.float(d), self.float(d));
        }
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

    /// A literal, mostly a plain one, else one whose result or printing is easy to get wrong.
    fn float_literal(&mut self) -> String {
        let plain = ["0.0", "1.0", "1.5", "2.5", "0.1", "3.0", "10.0", "0.5"];
        let tricky = [
            "(-0.0)",
            "(0.0 / 0.0)",
            "(1.0 / 0.0)",
            "(-1.0 / 0.0)",
            "0.49999999999999994",
            "(-2.5)",
            "4503599627370495.5",
            "1e308",
            "1.7976931348623157e308",
            "5e-324",
            "1e21",
            "1e-7",
            "9223372036854775807.0",
            "(-9223372036854775808.0)",
            "9223372036854774784.0",
        ];
        match self.rng.chance(70) {
            true => self.rng.pick(&plain).to_string(),
            false => self.rng.pick(&tricky).to_string(),
        }
    }

    fn float(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.chance(25) {
            return self.leaf(Ty::Float);
        }
        let d = depth - 1;
        loop {
            match self.rng.below(12) {
                0..=4 => {
                    let op = *self.rng.pick(&["+", "-", "*", "/", "%"]);
                    return format!("({} {op} {})", self.float(d), self.float(d));
                }
                5 => return format!("(-{})", self.float(d)),
                6 => {
                    let m = *self.rng.pick(&["abs", "floor", "ceil", "round", "sqrt"]);
                    return format!("({}).{m}()", self.float(d));
                }
                7 => return format!("({}).to_float()", self.int(d)),
                8 => {
                    let (x, y) = (self.float(d), self.float(d));
                    return format!("({x}).to_str().parse_float().unwrap_or({y})");
                }
                9 => return self.if_expr(Ty::Float, d),
                _ => {
                    if let Some(c) = self.call(Ty::Float) {
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

    /// A value of `S`, `K` or `Option<Int>`: a visible local, a construction, or a call of a
    /// helper returning `Option<Int>`. A bare `None` only here, where the context gives its type.
    fn adt(&mut self, ty: Ty, depth: u32) -> String {
        let vars = self.visible(|v| v.ty == ty);
        if !vars.is_empty() && self.rng.chance(40) {
            return self.rng.pick(&vars).clone();
        }
        let d = depth.saturating_sub(1);
        match ty {
            Ty::S => {
                let (a, b) = (self.int(d), self.bool(d));
                match self.rng.chance(50) {
                    true => format!("S {{ a: {a}, b: {b} }}"),
                    false => format!("S {{ b: {b}, a: {a} }}"),
                }
            }
            Ty::K => match self.rng.below(3) {
                0 => "K::Z".to_string(),
                1 => format!("K::One({})", self.int(d)),
                _ => format!("K::Two({}, {})", self.int(d), self.adt(Ty::Opt, d)),
            },
            _ if self.rng.chance(25) => "None".to_string(),
            _ => self.some(d),
        }
    }

    /// An `Option<Int>` that is never a bare `None`, for a receiver, an operand or a scrutinee.
    fn some(&mut self, depth: u32) -> String {
        let vars = self.visible(|v| v.ty == Ty::Opt);
        if !vars.is_empty() && self.rng.chance(50) {
            return self.rng.pick(&vars).clone();
        }
        if self.rng.chance(30) {
            if let Some(c) = self.call(Ty::Opt) {
                return c;
            }
        }
        format!("Some({})", self.int(depth))
    }

    /// A `K` for a scrutinee or an operand: a local or a construction.
    fn k(&mut self, depth: u32) -> String {
        self.adt(Ty::K, depth)
    }

    /// An `Int` from the ADTs: a field, `unwrap_or`, `?` in a function returning `Option<Int>`,
    /// a `match` or an `if let`.
    fn adt_int(&mut self, d: u32) -> String {
        let structs = self.visible(|v| v.ty == Ty::S);
        loop {
            match self.rng.below(6) {
                0 if !structs.is_empty() => return format!("{}.a", self.rng.pick(&structs).clone()),
                1 => return format!("({}).a", self.adt(Ty::S, d)),
                2 => return format!("{}.unwrap_or({})", self.some(d), self.int(d)),
                3 if self.ret == Ty::Opt => return format!("({}?)", self.some(d)),
                4 => return self.match_expr(d),
                5 => return self.if_let(d),
                _ => {}
            }
        }
    }

    fn adt_bool(&mut self, d: u32) -> String {
        let structs = self.visible(|v| v.ty == Ty::S);
        loop {
            match self.rng.below(5) {
                0 if !structs.is_empty() => return format!("{}.b", self.rng.pick(&structs).clone()),
                1 => {
                    let m = *self.rng.pick(&["is_some", "is_none"]);
                    return format!("{}.{m}()", self.some(d));
                }
                2 if structs.len() > 1 || (!structs.is_empty() && self.rng.chance(30)) => {
                    let (a, b) = (self.rng.pick(&structs).clone(), self.rng.pick(&structs).clone());
                    return format!("({a} == {b})");
                }
                3 => {
                    let op = *self.rng.pick(&["==", "!="]);
                    return format!("({} {op} {})", self.k(d), self.k(d));
                }
                4 => {
                    let op = *self.rng.pick(&["==", "!="]);
                    return format!("({} {op} {})", self.some(d), self.some(d));
                }
                _ => {}
            }
        }
    }

    /// Some of a fixed sequence of arms for a `K` or an `Option<Int>`, each more specific than or
    /// disjoint from the later ones, as patterns with `N` and `M` for names to bind; then the
    /// last arm: `_`, or `None` when an earlier arm covers every `Some`.
    fn arms(&mut self) -> (String, Vec<String>) {
        let lit = self.rng.below(4);
        let (scrut, cands) = match self.rng.chance(60) {
            true => (
                self.k(1),
                vec![
                    format!("K::One({lit})"),
                    format!("K::Two(N, Some({lit}))"),
                    "K::Two(N, Some(M))".to_string(),
                    "K::One(N)".to_string(),
                    "K::Z".to_string(),
                ],
            ),
            false => (self.some(1), vec![format!("Some({lit})"), "Some(N)".to_string()]),
        };
        let mut pats: Vec<String> = cands.into_iter().filter(|_| self.rng.chance(50)).collect();
        let last = match pats.last().map(String::as_str) {
            Some("Some(N)") if self.rng.chance(50) => "None",
            _ => "_",
        };
        pats.push(last.to_string());
        (scrut, pats)
    }

    /// `pat`, with each `N` and `M` (a name's place: after `(` or a space, before `)` or `,`)
    /// replaced by a fresh name, declared in the current scope.
    fn bind(&mut self, pat: &str) -> String {
        let b = pat.as_bytes();
        let mut out = String::new();
        for (i, c) in pat.char_indices() {
            let hole = matches!(c, 'N' | 'M')
                && i > 0
                && matches!(b[i - 1], b'(' | b' ')
                && matches!(b.get(i + 1), Some(b')' | b','));
            if hole {
                let name = self.name("n");
                out += &name;
                self.declare(name, Ty::Int, false, true);
            } else {
                out.push(c);
            }
        }
        out
    }

    /// `(match e { .. })` of `Int`, with nested patterns and a catch-all.
    fn match_expr(&mut self, d: u32) -> String {
        let (scrut, pats) = self.arms();
        let mut arms = Vec::new();
        for p in pats {
            self.scopes.push(Vec::new());
            let p = self.bind(&p);
            arms.push(format!("{p} => {}", self.int(d)));
            self.scopes.pop();
        }
        format!("(match {scrut} {{ {} }})", arms.join(", "))
    }

    /// `(if let PAT = e { .. } else { .. })` of `Int`.
    fn if_let(&mut self, d: u32) -> String {
        let (scrut, pats) = self.arms();
        self.scopes.push(Vec::new());
        let p = self.bind(&pats[0]);
        let then = self.int(d);
        self.scopes.pop();
        let els = self.int(d);
        format!("(if let {p} = {scrut} {{ {then} }} else {{ {els} }})")
    }

    /// A field write on a mutable `S`, or a `match` statement whose arms are blocks.
    fn adt_stmt(&mut self) -> Option<String> {
        let pad = self.pad();
        if self.rng.chance(50) {
            let structs = self.visible(|v| v.ty == Ty::S && v.mutable && !v.fixed);
            if structs.is_empty() {
                return None;
            }
            let s = self.rng.pick(&structs).clone();
            return Some(match self.rng.below(3) {
                0 => format!("{pad}{s}.b = {};\n", self.bool(2)),
                _ => {
                    let op = *self.rng.pick(&["=", "+=", "-="]);
                    format!("{pad}{s}.a {op} {};\n", self.int(2))
                }
            });
        }
        if self.nest >= MAX_NEST {
            return None;
        }
        let (scrut, pats) = self.arms();
        let mut arms = Vec::new();
        for p in pats {
            self.scopes.push(Vec::new());
            let p = self.bind(&p);
            arms.push(format!("{pad}    {p} => {},\n", self.block(2, Ty::Unit)));
            self.scopes.pop();
        }
        Some(format!("{pad}match {scrut} {{\n{}{pad}}}\n", arms.concat()))
    }

    /// A `Str` expression and a bound on its length. Inside a loop it reads no accumulator.
    fn str_expr(&mut self, depth: u32) -> (String, u64) {
        self.cost += self.mult;
        if depth == 0 || self.rng.chance(30) {
            return self.str_leaf();
        }
        let d = depth - 1;
        loop {
            match self.rng.below(10) {
                0..=3 => {
                    let ((a, x), (b, y)) = (self.str_expr(d), self.str_expr(d));
                    return (format!("({a} + {b})"), x + y);
                }
                4 => return (format!("({}).to_str()", self.int(d)), 20),
                5 => {
                    let c = self.bool(d);
                    let ((a, x), (b, y)) = (self.str_expr(d), self.str_expr(d));
                    return (format!("(if {c} {{ {a} }} else {{ {b} }})"), x.max(y));
                }
                6 => {
                    let lists = self.visible(|v| v.ty == Ty::StrList);
                    if !lists.is_empty() {
                        let ws = self.rng.pick(&lists).clone();
                        let i = self.index(&ws);
                        return (format!("{ws}[{i}]"), self.var(&ws).smax);
                    }
                }
                7 => {
                    if let Some(c) = self.call(Ty::Str) {
                        return (c, RET_MAX);
                    }
                }
                8 => {
                    let (a, x) = self.str_expr(d);
                    return (format!("({a}).trim()"), x);
                }
                _ => return self.str_leaf(),
            }
        }
    }

    /// A visible `Str` (no accumulator inside a loop) or a literal.
    fn str_leaf(&mut self) -> (String, u64) {
        let in_loop = self.loops > 0;
        let vars = self.visible(|v| v.ty == Ty::Str && !(in_loop && v.acc));
        if !vars.is_empty() && self.rng.chance(50) {
            let n = self.rng.pick(&vars).clone();
            let m = self.var(&n).smax;
            return (n, m);
        }
        let (s, n) = *self.rng.pick(STR_LITS);
        (s.to_string(), n)
    }

    /// A `Str` expression of at most `bound` bytes.
    fn str_upto(&mut self, depth: u32, bound: u64) -> String {
        for _ in 0..3 {
            let (s, n) = self.str_expr(depth);
            if n <= bound {
                return s;
            }
        }
        "\"q\"".to_string()
    }

    /// A `Str` that is read and not kept, so it may be any visible one.
    fn str_read(&mut self, depth: u32) -> String {
        let vars = self.visible(|v| v.ty == Ty::Str);
        if !vars.is_empty() && self.rng.chance(40) {
            return self.rng.pick(&vars).clone();
        }
        self.str_expr(depth).0
    }

    /// A separator for `split`: mostly a non-empty literal.
    fn sep(&mut self) -> String {
        if self.rng.chance(5) {
            return self.str_read(0);
        }
        self.rng
            .pick(&["\",\"", "\" \"", "\"a\"", "\"ab\"", "\"\\n\""])
            .to_string()
    }

    /// An `Int` from strings: a length, a count of parts, or a `match` on literals.
    fn str_int(&mut self, d: u32) -> String {
        let lists = self.visible(|v| v.ty == Ty::StrList);
        match self.rng.below(6) {
            0 | 1 => format!("({}).len()", self.str_read(d)),
            2 if !lists.is_empty() => format!("{}.len()", self.rng.pick(&lists)),
            2 | 3 => format!("({}).split({}).len()", self.str_read(d), self.sep()),
            4 => format!("({}).lines().len()", self.str_read(d)),
            _ => {
                let scrut = self.str_read(d);
                let mut arms = Vec::new();
                for (lit, _) in STR_LITS {
                    if self.rng.chance(25) {
                        arms.push(format!("{lit} => {}", self.int(d)));
                    }
                }
                arms.push(format!("_ => {}", self.int(d)));
                format!("(match {scrut} {{ {} }})", arms.join(", "))
            }
        }
    }

    /// A `Bool` from strings: `==`, `!=`, `contains` on a `Str` or a `List<Str>`.
    fn str_bool(&mut self, d: u32) -> String {
        let lists = self.visible(|v| v.ty == Ty::StrList);
        match self.rng.below(4) {
            0 | 1 => {
                let op = *self.rng.pick(&["==", "!="]);
                format!("({} {op} {})", self.str_read(d), self.str_read(d))
            }
            2 if !lists.is_empty() => {
                let ws = self.rng.pick(&lists).clone();
                format!("{ws}.contains({})", self.str_read(d))
            }
            _ => format!("({}).contains({})", self.str_read(d), self.str_read(d)),
        }
    }

    /// A `Str` or `List<Str>` statement: a `let`, an assignment or append, or a `for` over parts.
    fn str_stmt(&mut self) -> Option<String> {
        let pad = self.pad();
        match self.rng.below(10) {
            0..=2 => {
                let (e, max) = self.str_expr(MAX_DEPTH);
                if max > MAX_STR {
                    return None;
                }
                let name = self.name("s");
                let mutable = self.rng.chance(70);
                let annot = if self.rng.chance(30) { ": Str" } else { "" };
                let m = if mutable { "mut " } else { "" };
                self.declare(name.clone(), Ty::Str, mutable, false);
                let acc = mutable && self.rng.chance(50);
                let v = self.var(&name);
                (v.smax, v.acc) = (max, acc);
                Some(format!("{pad}let {m}{name}{annot} = {e};\n"))
            }
            3..=6 => self.str_assign(),
            7 => {
                let (s, max) = self.str_expr(1);
                let (src, min) = match self.rng.chance(60) {
                    true => (format!("({s}).split({})", self.sep()), 1),
                    false => (format!("({s}).lines()"), 0),
                };
                let name = self.name("ws");
                self.declare(name.clone(), Ty::StrList, false, false);
                let v = self.var(&name);
                (v.min, v.max, v.smax) = (min, max + 1, max);
                Some(format!("{pad}let {name} = {src};\n"))
            }
            _ => self.str_for(),
        }
    }

    /// An assignment to a mutable `Str`. Inside a loop, only an append of small parts to an
    /// accumulator, whose bound grows by the parts' times the passes.
    fn str_assign(&mut self) -> Option<String> {
        let in_loop = self.loops > 0;
        let names = self.visible(|v| v.ty == Ty::Str && v.mutable && !v.fixed && (!in_loop || v.acc));
        if names.is_empty() {
            return None;
        }
        let name = self.rng.pick(&names).clone();
        let cur = self.var(&name).smax;
        let (stmt, new) = if in_loop {
            let parts: Vec<(String, u64)> = (0..1 + self.rng.below(2)).map(|_| self.str_expr(1)).collect();
            let add: u64 = parts.iter().map(|p| p.1).sum();
            let s = match parts.len() == 1 && self.rng.chance(50) {
                true => format!("{name} += {};", parts[0].0),
                false => {
                    let ps: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
                    format!("{name} = {name} + {};", ps.join(" + "))
                }
            };
            (s, cur.saturating_add(self.mult.saturating_mul(add)))
        } else {
            match self.rng.below(5) {
                0 => {
                    let (p, n) = self.str_expr(2);
                    (format!("{name} += {p};"), cur + n)
                }
                1 => {
                    let ((a, x), (b, y)) = (self.str_expr(1), self.str_expr(1));
                    (format!("{name} = {name} + {a} + {b};"), cur + x + y)
                }
                2 => (format!("{name} = {name} + {name};"), 2 * cur),
                3 => (format!("{name} = {name} + \"-\" + {name};"), 2 * cur + 1),
                _ => {
                    let (e, n) = self.str_expr(MAX_DEPTH);
                    (format!("{name} = {e};"), cur.max(n))
                }
            }
        };
        if new > MAX_STR {
            return None;
        }
        self.var(&name).smax = new;
        Some(format!("{}{stmt}\n", self.pad()))
    }

    /// `for w in ws` over a `List<Str>` local, or over a `split` or `lines` of a `Str`; its
    /// passes are bounded by the parts' count, at most the length plus one.
    fn str_for(&mut self) -> Option<String> {
        if !self.features.loops || self.loops >= MAX_LOOPS || self.nest >= MAX_NEST || self.cost > BUDGET / 2 {
            return None;
        }
        let lists = self.visible(|v| v.ty == Ty::StrList);
        let (src, runs, smax) = if !lists.is_empty() && self.rng.chance(50) {
            let ws = self.rng.pick(&lists).clone();
            let v = self.var(&ws);
            let (runs, smax) = (v.max, v.smax);
            (ws, runs, smax)
        } else {
            let (s, m) = self.str_expr(1);
            let src = match self.rng.chance(60) {
                true => format!("({s}).split({})", self.sep()),
                false => format!("({s}).lines()"),
            };
            (src, m + 1, m)
        };
        let cap = if self.loops > 0 { 65 } else { MAX_STR + 1 };
        if runs > cap {
            return None;
        }
        let runs = runs.max(1);
        let pad = self.pad();
        let w = self.name("w");
        self.mult *= runs;
        self.loops += 1;
        self.scopes.push(vec![]);
        self.declare(w.clone(), Ty::Str, false, true);
        self.var(&w).smax = smax;
        let body = self.block(3, Ty::Unit);
        self.scopes.pop();
        self.loops -= 1;
        self.mult /= runs;
        Some(format!("{pad}for {w} in {src} {body}\n"))
    }
}

#[cfg(test)]
mod tests {
    /// Floats appear in a run of the default size, with their methods and tricky literals.
    #[test]
    fn floats_are_generated() {
        let all: String = (0..250).map(|s| super::program(s, &Default::default())).collect();
        for m in [
            ": Float",
            ".round()",
            ".to_int()",
            ".parse_float()",
            "0.49999999999999994",
            " % ",
            "-> Float",
        ] {
            assert!(all.matches(m).count() > 10, "{m}: {}", all.matches(m).count());
        }
    }

    /// The list methods appear in a run of the default size.
    #[test]
    fn list_methods_are_generated() {
        let all: String = (0..250).map(|s| super::program(s, &Default::default())).collect();
        for m in [".sort()", ".pop()", ".contains("] {
            assert!(all.matches(m).count() > 10, "{m}: {}", all.matches(m).count());
        }
    }

    /// The built-ins taking closures appear in a run of the default size.
    #[test]
    fn closures_are_generated() {
        let all: String = (0..250).map(|s| super::program(s, &Default::default())).collect();
        for m in [".map(|el| ", ".filter(|el| ", ".any(|el| ", ".all(|el| "] {
            assert!(all.matches(m).count() > 10, "{m}: {}", all.matches(m).count());
        }
    }

    /// Strings appear in a run of the default size, with the in-place append's patterns.
    #[test]
    fn strings_are_generated() {
        let all: String = (0..250).map(|s| super::program(s, &Default::default())).collect();
        for m in [
            "let mut s",
            " += ",
            ".split(",
            ".lines()",
            ".contains(",
            ").to_str()",
            ").len()",
            "-> Str",
            ": Str",
            "for w",
            "\"prefix-shared-key-1\" => ",
        ] {
            assert!(all.matches(m).count() > 10, "{m}: {}", all.matches(m).count());
        }
        // `s = s + ..` chains, and `s = s + s`.
        let chain = all.lines().filter(|l| {
            let l = l.trim();
            l.split_once(" = ")
                .is_some_and(|(x, e)| x.starts_with('s') && e.starts_with(&format!("{x} + ")))
        });
        let n = chain.count();
        assert!(n > 30, "{n}");
    }
}
