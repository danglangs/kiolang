//! Programs over the one-slot enums whose payload is a pointer (C8c1 unboxes them):
//! `Option<Str>`, `Result<Str, Int>`, the mixed `enum J { Null, N(Int), S(Str) }`,
//! `Option<Option<Str>>`, `List<Option<Str>>` (`push`, `pop`) and `Map<Str, Option<Str>>`
//! (`insert`, `get`), built, passed to and returned from helpers, matched, compared, used
//! with `?` and through closures (`map`, `and_then`, `unwrap_or_else`, ..). Ported from the
//! C8c1 review's generator.
//!
//! Every program terminates: loops are `for` over at most 4 values, nested at most 3 deep,
//! and helpers call no helper. Every `Str` has a bound on its length (`G::bound`): a value
//! stored is at most `CAP` bytes and an argument to a helper at most `PARAM_MAX` (otherwise
//! a literal is stored or passed), and `out` is appended to only while it stays within
//! `OUT_CAP`. A loop body reads no mutable non-`Int` local declared outside it, so nothing
//! doubles per pass.

use crate::Rng;

#[derive(Clone, Copy, PartialEq)]
enum T {
    Int,
    Str,
    /// `Option<Str>`
    Os,
    /// `Result<Str, Int>`
    Rs,
    J,
    /// `Option<Option<Str>>`
    Oos,
    /// `List<Option<Str>>`, a local only.
    List,
    /// `Map<Str, Option<Str>>`, a local only.
    Map,
}

impl T {
    fn name(self) -> &'static str {
        match self {
            T::Int => "Int",
            T::Str => "Str",
            T::Os => "Option<Str>",
            T::Rs => "Result<Str, Int>",
            T::J => "J",
            T::Oos => "Option<Option<Str>>",
            T::List => "List<Option<Str>>",
            T::Map => "Map<Str, Option<Str>>",
        }
    }
}

const VALUES: [T; 6] = [T::Int, T::Str, T::Os, T::Rs, T::J, T::Oos];
const CAP: u64 = 4096;
const PARAM_MAX: u64 = 256;
const OUT_CAP: u64 = 1 << 16;
const HELPER_OUT_CAP: u64 = 1024;
const INT_STR: u64 = 20;

#[derive(Clone)]
struct Var {
    name: String,
    ty: T,
    mutable: bool,
    /// False inside a loop for a mutable non-`Int` local declared outside it.
    readable: bool,
    /// Index into `G::bound`.
    id: usize,
}

type Scope = Vec<Var>;

/// An expression and a bound on the length of the `Str`s it holds.
type E = (String, u64);

struct G {
    rng: Rng,
    n: u32,
    in_helper: bool,
    /// Per variable (for a list or map, per element), shared by every copy of a scope:
    /// assignments in a branch only raise it.
    bound: Vec<u64>,
    /// `out`'s bound in the function being generated, and its cap.
    out: u64,
    out_cap: u64,
    /// Passes of the current statement per call.
    mult: u64,
    /// Result bounds of `h_os`, `h_rs`, `h_j`, `h_oos`.
    helper: [u64; 4],
}

pub fn program(seed: u64) -> String {
    let mut g = G {
        rng: Rng::new(seed),
        n: 0,
        in_helper: false,
        bound: Vec::new(),
        out: 0,
        out_cap: HELPER_OUT_CAP,
        mult: 1,
        helper: [0; 4],
    };
    let mut src = String::from("enum J { Null, N(Int), S(Str) }\n");
    src += "fn mk_some(s: Str) -> Option<Str> { Some(s) }\n";
    src += "fn mk_none() -> Option<Str> { None }\n";
    src += "fn mk_ok(s: Str) -> Result<Str, Int> { Ok(s) }\n";
    src += "fn mk_err(n: Int) -> Result<Str, Int> { Err(n) }\n";
    src += "fn mk_soos(o: Option<Str>) -> Option<Option<Str>> { Some(o) }\n";
    src += "fn mk_noos() -> Option<Option<Str>> { None }\n";
    src += &g.func(0, "h_os", &[("a", T::Os), ("k", T::Int)], T::Os);
    src += &g.func(1, "h_rs", &[("a", T::Rs), ("t", T::Str)], T::Rs);
    src += &g.func(2, "h_j", &[("a", T::J), ("o", T::Os)], T::J);
    src += &g.func(3, "h_oos", &[("a", T::Oos)], T::Oos);
    g.out = 0;
    g.out_cap = OUT_CAP;
    let mut scope = Scope::new();
    let n = 6 + g.rng.below(9) as usize;
    let body = g.stmts(&mut scope, 3, T::Str, false, n);
    src += "fn main() -> Str {\n    let mut out: Str = \"\" + \"\";\n    ";
    src += &body.join("\n    ");
    src += "\n    out\n}\n";
    src
}

fn simple(x: &str) -> bool {
    let first = x.chars().next().unwrap_or(' ');
    first == '('
        || first == '"'
        || x.chars().all(|c| c.is_ascii_digit())
        || (!first.is_ascii_digit() && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

impl G {
    fn name(&mut self, p: &str) -> String {
        self.n += 1;
        format!("{p}{}", self.n)
    }

    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.rng.below((hi - lo + 1) as u64) as i64
    }

    fn declare(&mut self, scope: &mut Scope, name: &str, ty: T, mutable: bool, bound: u64) {
        self.bound.push(bound);
        let id = self.bound.len() - 1;
        let name = name.to_string();
        scope.push(Var {
            name,
            ty,
            mutable,
            readable: true,
            id,
        });
    }

    /// The readable locals of type `t`, innermost binding of each name.
    fn locals(scope: &Scope, t: T, readable_only: bool) -> Vec<Var> {
        let mut out: Vec<Var> = Vec::new();
        for v in scope.iter().rev() {
            if v.ty == t && (v.readable || !readable_only) && !out.iter().any(|o| o.name == v.name) {
                out.push(v.clone());
            }
        }
        out
    }

    fn pick_var(&mut self, vs: &[Var]) -> Var {
        self.rng.pick(vs).clone()
    }

    fn e(&mut self, t: T, scope: &Scope, d: i32, f: T) -> E {
        let (x, b) = self.e0(t, scope, d, f);
        if simple(&x) {
            (x, b)
        } else {
            (format!("({x})"), b)
        }
    }

    /// A literal of type `t`: reads no local.
    fn lit(&mut self, t: T) -> E {
        let r = &mut self.rng;
        match t {
            T::Int => ((r.below(15) as i64 - 5).to_string(), 0),
            T::Str => (format!("(\"{}\" + \"\")", r.pick(&["a", "bb", "ccc", "", "dd"])), 3),
            T::Os => {
                if r.chance(50) {
                    ("mk_none()".into(), 0)
                } else {
                    (format!("mk_some(\"{}\" + \"\")", r.pick(&["x", "yy", ""])), 2)
                }
            }
            T::Rs => {
                if r.chance(50) {
                    ("mk_err(3)".into(), 0)
                } else {
                    (format!("mk_ok(\"{}\" + \"\")", r.pick(&["o", "pp"])), 2)
                }
            }
            T::J => (r.pick(&["J::Null", "J::N(2)", "J::S(\"js\" + \"\")"]).to_string(), 2),
            T::Oos => {
                let x = r.pick(&["mk_noos()", "mk_soos(mk_none())", "mk_soos(mk_some(\"q\" + \"\"))"]);
                (x.to_string(), 1)
            }
            T::List | T::Map => unreachable!(),
        }
    }

    fn leaf(&mut self, t: T, scope: &Scope) -> E {
        let vs = Self::locals(scope, t, true);
        if !vs.is_empty() && self.rng.chance(60) {
            let v = self.pick_var(&vs);
            return (v.name, self.bound[v.id]);
        }
        self.lit(t)
    }

    /// An argument to a helper: at most `PARAM_MAX` bytes.
    fn arg(&mut self, t: T, scope: &Scope, d: i32, f: T) -> E {
        let a = self.e(t, scope, d, f);
        if a.1 > PARAM_MAX {
            self.lit(t)
        } else {
            a
        }
    }

    fn e0(&mut self, t: T, scope: &Scope, d: i32, f: T) -> E {
        if d <= 0 {
            return self.leaf(t, scope);
        }
        let d1 = d - 1;
        match t {
            T::Int => match self.rng.below(7) {
                0 => self.leaf(t, scope),
                1 => (format!("({}).len()", self.e(T::Str, scope, d1, f).0), 0),
                2 => {
                    let o = self.e(T::Os, scope, d1, f).0;
                    (format!("{o}.map(|s: Str| s.len()).unwrap_or({})", self.int(-3, 3)), 0)
                }
                3 => {
                    let r = self.e(T::Rs, scope, d1, f).0;
                    (format!("match {r} {{ Ok(s) => s.len(), Err(n) => n }}"), 0)
                }
                4 => {
                    let a = self.e(T::Int, scope, d1, f).0;
                    let b = self.e(T::Int, scope, d1, f).0;
                    (format!("({a} + {b})"), 0)
                }
                5 => {
                    let j = self.e(T::J, scope, d1, f).0;
                    (
                        format!("match {j} {{ J::Null => 0, J::N(n) => n, J::S(s) => s.len() }}"),
                        0,
                    )
                }
                _ => {
                    let o = self.e(T::Oos, scope, d1, f).0;
                    let m = "map(|o: Option<Str>| o.map(|s: Str| s.len()).unwrap_or(-1))";
                    (format!("{o}.{m}.unwrap_or(-2)"), 0)
                }
            },
            T::Str => match self.rng.below(9) {
                0 => self.leaf(t, scope),
                1 => {
                    let (a, x) = self.e(T::Str, scope, d1, f);
                    let (b, y) = self.e(T::Str, scope, d1, f);
                    (format!("({a} + {b})"), x + y)
                }
                2 => {
                    let (a, x) = self.e(T::Os, scope, d1, f);
                    let (b, y) = self.e(T::Str, scope, d1, f);
                    (format!("{a}.unwrap_or({b})"), x.max(y))
                }
                3 => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    let (b, y) = self.e(T::Str, scope, d1, f);
                    (format!("{a}.unwrap_or({b})"), x.max(y))
                }
                4 => {
                    let (j, x) = self.e(T::J, scope, d1, f);
                    let arms = "J::Null => \"nul\", J::N(n) => n.to_str(), J::S(s) => s";
                    (format!("match {j} {{ {arms} }}"), x.max(INT_STR))
                }
                5 => (format!("{}.to_str()", self.e(T::Int, scope, d1, f).0), INT_STR),
                6 => {
                    let (o, x) = self.e(T::Oos, scope, d1, f);
                    let arms = "Some(Some(s)) => s, Some(None) => \"sn\", None => \"n\"";
                    (format!("match {o} {{ {arms} }}"), x.max(2))
                }
                7 => {
                    let (a, x) = self.e(T::Os, scope, d1, f);
                    let (b, y) = self.e(T::Str, scope, d1, f);
                    (format!("{a}.unwrap_or_else(|| {b})"), x.max(y))
                }
                _ => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    (format!("{a}.unwrap_or_else(|n: Int| n.to_str())"), x.max(INT_STR))
                }
            },
            T::Os => match self.rng.below(12) {
                0 => self.leaf(t, scope),
                1 => {
                    let (s, x) = self.e(T::Str, scope, d1, f);
                    (format!("mk_some({s})"), x)
                }
                2 => {
                    let (a, x) = self.e(T::Os, scope, d1, f);
                    let (b, y) = self.e(T::Str, scope, d1, f);
                    (format!("{a}.map(|s: Str| s + {b})"), x + y)
                }
                3 => {
                    let (a, x) = self.e(T::Os, scope, d1, f);
                    let n = self.rng.below(7);
                    (
                        format!("{a}.and_then(|s: Str| if s.len() > {n} {{ Some(s) }} else {{ None }})"),
                        x,
                    )
                }
                4 => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    (format!("{a}.ok()"), x)
                }
                5 => {
                    let (a, x) = self.e(T::Str, scope, d1, f);
                    let (b, _) = self.e(T::Str, scope, d1, f);
                    (format!("({a}).strip_prefix({b})"), x)
                }
                6 => self.if_else(t, scope, d1, f),
                7 => {
                    let (a, x) = self.e(T::Oos, scope, d1, f);
                    let (b, y) = self.e(T::Os, scope, d1, f);
                    (format!("{a}.unwrap_or({b})"), x.max(y))
                }
                8 => {
                    let (j, x) = self.e(T::J, scope, d1, f);
                    let arms = "J::Null => None, J::N(n) => Some(n.to_str()), J::S(s) => Some(s)";
                    (format!("match {j} {{ {arms} }}"), x.max(INT_STR))
                }
                9 => {
                    let (a, x) = self.e(T::Oos, scope, d1, f);
                    (format!("{a}.and_then(|o: Option<Str>| o)"), x)
                }
                10 if !self.in_helper => {
                    let (a, _) = self.arg(T::Os, scope, d1, f);
                    let (k, _) = self.e(T::Int, scope, d1, f);
                    (format!("h_os({a}, {k})"), self.helper[0])
                }
                _ => {
                    let (a, _) = self.e(T::Rs, scope, d1, f);
                    (format!("{a}.err().map(|n: Int| n.to_str())"), INT_STR)
                }
            },
            T::Rs => match self.rng.below(9) {
                0 => self.leaf(t, scope),
                1 => {
                    let (s, x) = self.e(T::Str, scope, d1, f);
                    (format!("mk_ok({s})"), x)
                }
                2 => (format!("mk_err({})", self.e(T::Int, scope, d1, f).0), 0),
                3 => {
                    let (a, x) = self.e(T::Os, scope, d1, f);
                    let (n, _) = self.e(T::Int, scope, d1, f);
                    (format!("{a}.ok_or({n})"), x)
                }
                4 => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    (format!("{a}.map(|s: Str| s + \"m\")"), x + 1)
                }
                5 => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    (format!("{a}.map_err(|n: Int| n + 1)"), x)
                }
                6 => {
                    let (a, x) = self.e(T::Rs, scope, d1, f);
                    let c = "if s.len() % 2 == 0 { Ok(s + \"a\") } else { Err(s.len()) }";
                    (format!("{a}.and_then(|s: Str| {c})"), x + 1)
                }
                7 if !self.in_helper => {
                    let (a, _) = self.arg(T::Rs, scope, d1, f);
                    let (s, _) = self.arg(T::Str, scope, d1, f);
                    (format!("h_rs({a}, {s})"), self.helper[1])
                }
                _ => self.if_else(t, scope, d1, f),
            },
            T::J => match self.rng.below(7) {
                0 => self.leaf(t, scope),
                1 => (format!("J::N({})", self.e(T::Int, scope, d1, f).0), 0),
                2 => {
                    let (s, x) = self.e(T::Str, scope, d1, f);
                    (format!("J::S({s})"), x)
                }
                3 => ("J::Null".into(), 0),
                4 if !self.in_helper => {
                    let (a, _) = self.arg(T::J, scope, d1, f);
                    let (o, _) = self.arg(T::Os, scope, d1, f);
                    (format!("h_j({a}, {o})"), self.helper[2])
                }
                5 => {
                    let (o, x) = self.e(T::Os, scope, d1, f);
                    let n = self.rng.below(10);
                    (format!("match {o} {{ Some(s) => J::S(s), None => J::N({n}) }}"), x)
                }
                _ => self.if_else(t, scope, d1, f),
            },
            T::Oos => match self.rng.below(6) {
                0 => self.leaf(t, scope),
                1 => {
                    let (o, x) = self.e(T::Os, scope, d1, f);
                    (format!("mk_soos({o})"), x)
                }
                2 => {
                    let (o, x) = self.e(T::Os, scope, d1, f);
                    (
                        format!("{o}.map(|s: Str| if s.len() > 2 {{ Some(s) }} else {{ None }})"),
                        x,
                    )
                }
                3 => self.if_else(t, scope, d1, f),
                4 => {
                    let maps = Self::locals(scope, T::Map, true);
                    if maps.is_empty() {
                        let (o, x) = self.e(T::Os, scope, d1, f);
                        return (format!("mk_soos({o})"), x);
                    }
                    let m = self.pick_var(&maps);
                    let (k, _) = self.e(T::Str, scope, d1, f);
                    (format!("{}.get({k})", m.name), self.bound[m.id])
                }
                _ if !self.in_helper => {
                    let (a, _) = self.arg(T::Oos, scope, d1, f);
                    (format!("h_oos({a})"), self.helper[3])
                }
                _ => self.leaf(t, scope),
            },
            T::List | T::Map => unreachable!(),
        }
    }

    fn if_else(&mut self, t: T, scope: &Scope, d: i32, f: T) -> E {
        let c = self.cond(scope, d, f);
        let (a, x) = self.e(t, scope, d, f);
        let (b, y) = self.e(t, scope, d, f);
        (format!("if {c} {{ {a} }} else {{ {b} }}"), x.max(y))
    }

    fn cond(&mut self, scope: &Scope, d: i32, f: T) -> String {
        match self.rng.below(5) {
            0 => format!("{}.is_some()", self.e(T::Os, scope, d, f).0),
            1 => format!("{}.is_err()", self.e(T::Rs, scope, d, f).0),
            2 => format!("{} % 2 == 0", self.e(T::Int, scope, d, f).0),
            3 => {
                let a = self.e(T::Os, scope, d, f).0;
                format!("{a} == {}", self.e(T::Os, scope, d, f).0)
            }
            _ => {
                let a = self.e(T::J, scope, d, f).0;
                format!("{a} != {}", self.e(T::J, scope, d, f).0)
            }
        }
    }

    fn stmts(&mut self, scope: &mut Scope, d: i32, f: T, in_loop: bool, n: usize) -> Vec<String> {
        (0..n).map(|_| self.stmt(scope, d, f, in_loop)).collect()
    }

    /// A block's statements in a copy of `scope`.
    fn block(&mut self, scope: &Scope, d: i32, f: T, in_loop: bool, n: usize) -> String {
        let mut inner = scope.clone();
        self.stmts(&mut inner, d, f, in_loop, n).join(" ")
    }

    /// `out = out + e;`, or a literal when `e` could take `out` past its cap.
    fn append(&mut self, e: E) -> String {
        let (mut x, mut b) = e;
        if self.out + b * self.mult > self.out_cap {
            (x, b) = self.lit(T::Str);
        }
        self.out += b * self.mult;
        format!("out = out + {x};")
    }

    /// `e`, or a literal when its bound is over `CAP`.
    fn stored(&mut self, t: T, e: E) -> E {
        if e.1 > CAP {
            self.lit(t)
        } else {
            e
        }
    }

    fn stmt(&mut self, scope: &mut Scope, d: i32, f: T, in_loop: bool) -> String {
        let k = self.rng.below(12);
        match k {
            0..=2 => {
                let t = *self.rng.pick(&VALUES);
                let v = self.name("v");
                let mutable = self.rng.chance(50);
                let e = self.e(t, scope, d, f);
                let (init, b) = self.stored(t, e);
                self.declare(scope, &v, t, mutable, b);
                let m = if mutable { "mut " } else { "" };
                format!("let {m}{v}: {} = {init};", t.name())
            }
            3 => {
                let muts: Vec<Var> = Self::locals(scope, T::Int, false)
                    .into_iter()
                    .chain(VALUES[1..].iter().flat_map(|&t| Self::locals(scope, t, false)))
                    .filter(|v| v.mutable)
                    .collect();
                if muts.is_empty() {
                    let e = self.e(T::Str, scope, d, f);
                    return self.append(e);
                }
                let v = self.pick_var(&muts);
                let e = self.e(v.ty, scope, d, f);
                let (x, b) = self.stored(v.ty, e);
                self.bound[v.id] = self.bound[v.id].max(b);
                format!("{} = {x};", v.name)
            }
            5 => {
                let c = self.cond(scope, d - 1, f);
                let n1 = 1 + self.rng.below(3) as usize;
                let a = self.block(scope, d - 1, f, in_loop, n1);
                let n2 = 1 + self.rng.below(2) as usize;
                let b = self.block(scope, d - 1, f, in_loop, n2);
                format!("if {c} {{ {a} }} else {{ {b} }}")
            }
            6 if d > 0 => {
                let i = self.name("i");
                let mut inner: Scope = scope
                    .iter()
                    .map(|v| {
                        let mut v = v.clone();
                        if v.mutable && !matches!(v.ty, T::Int | T::List) {
                            v.readable = false;
                        }
                        v
                    })
                    .collect();
                self.declare(&mut inner, &i, T::Int, false, 0);
                let passes = 1 + self.rng.below(4);
                self.mult *= passes;
                let n = 1 + self.rng.below(3) as usize;
                let body = self.stmts(&mut inner, d - 1, f, true, n).join(" ");
                let mut exit = String::new();
                if self.rng.chance(60) {
                    let c = self.cond(&inner, 0, f);
                    let x = self.rng.pick(&["break;", "continue;"]);
                    exit = format!(" if {c} {{ {x} }}");
                }
                self.mult /= passes;
                format!("for {i} in 0..{passes} {{ {body}{exit} }}")
            }
            7 => self.match_stmt(scope, d, f, in_loop),
            8 => {
                let lists = Self::locals(scope, T::List, false);
                if !lists.is_empty() {
                    let l = self.pick_var(&lists);
                    let e = self.e(T::Os, scope, d, f);
                    let (x, b) = self.stored(T::Os, e);
                    self.bound[l.id] = self.bound[l.id].max(b);
                    return format!("{}.push({x});", l.name);
                }
                let v = self.name("xs");
                let e = self.e(T::Os, scope, d, f);
                let (a, x) = self.stored(T::Os, e);
                let e = self.e(T::Os, scope, d, f);
                let (b, y) = self.stored(T::Os, e);
                self.declare(scope, &v, T::List, true, x.max(y));
                format!("let mut {v}: List<Option<Str>> = [{a}, {b}];")
            }
            9 => {
                let maps = Self::locals(scope, T::Map, false);
                if !maps.is_empty() {
                    let m = self.pick_var(&maps);
                    let (key, _) = self.e(T::Str, scope, 0, f);
                    let e = self.e(T::Os, scope, d, f);
                    let (x, b) = self.stored(T::Os, e);
                    self.bound[m.id] = self.bound[m.id].max(b);
                    return format!("{}.insert({key}, {x});", m.name);
                }
                let v = self.name("m");
                self.declare(scope, &v, T::Map, true, 0);
                format!("let mut {v}: Map<Str, Option<Str>> = Map::new();")
            }
            10 => {
                let lists = Self::locals(scope, T::List, false);
                if lists.is_empty() {
                    let e = self.e(T::Str, scope, d, f);
                    return self.append(e);
                }
                let l = self.pick_var(&lists);
                let some = self.append(("s".into(), self.bound[l.id]));
                format!(
                    "match {}.pop() {{ Some(Some(s)) => {{ {some} }} Some(None) => {{ out = out + \"pn\"; }} None => {{ out = out + \"pe\"; }} }}",
                    l.name
                )
            }
            11 if matches!(f, T::Os | T::Rs) && self.rng.chance(50) => {
                let v = self.name("q");
                let e = self.e(f, scope, d, f);
                let (init, b) = self.stored(f, e);
                self.declare(scope, &v, T::Str, false, b);
                format!("let {v}: Str = {init}?;")
            }
            11 if in_loop && self.rng.chance(30) => {
                let c = self.cond(scope, 0, f);
                let x = self.rng.pick(&["break;", "continue;"]);
                format!("if {c} {{ {x} }}")
            }
            _ => {
                let e = self.e(T::Str, scope, d, f);
                self.append(e)
            }
        }
    }

    fn match_stmt(&mut self, scope: &Scope, d: i32, f: T, in_loop: bool) -> String {
        let t = *self.rng.pick(&[T::Os, T::Rs, T::J, T::Oos]);
        let (scrut, b) = self.e(t, scope, d - 1, f);
        let mut arms = Vec::new();
        // An arm binding a `Str` (bound `b`) or an `Int`, then its statements.
        let mut arm = |g: &mut G, pat: &str, bind: Option<(&str, T)>, most: u64| {
            let mut inner = scope.clone();
            let mut pat = pat.to_string();
            if let Some((p, ty)) = bind {
                let v = g.name(p);
                pat = pat.replace('@', &v);
                g.declare(&mut inner, &v, ty, false, if ty == T::Str { b } else { 0 });
            }
            let n = 1 + g.rng.below(most) as usize;
            let body = g.stmts(&mut inner, d - 1, f, in_loop, n).join(" ");
            arms.push(format!("{pat} => {{ {body} }}"));
        };
        match t {
            T::Os => {
                arm(self, "Some(@)", Some(("s", T::Str)), 2);
                arm(self, "None", None, 1);
            }
            T::Rs => {
                arm(self, "Ok(@)", Some(("s", T::Str)), 2);
                arm(self, "Err(@)", Some(("n", T::Int)), 1);
            }
            T::J => {
                arm(self, "J::Null", None, 1);
                arm(self, "J::N(@)", Some(("n", T::Int)), 1);
                arm(self, "J::S(@)", Some(("s", T::Str)), 2);
            }
            _ => {
                arm(self, "Some(Some(@))", Some(("s", T::Str)), 2);
                arm(self, "Some(None)", None, 1);
                arm(self, "None", None, 1);
            }
        }
        format!("match {scrut} {{ {} }}", arms.join(" "))
    }

    fn func(&mut self, i: usize, name: &str, params: &[(&str, T)], ret: T) -> String {
        self.in_helper = true;
        self.out = 0;
        self.out_cap = HELPER_OUT_CAP;
        let mut scope = Scope::new();
        for &(p, t) in params {
            self.declare(&mut scope, p, t, false, PARAM_MAX);
        }
        let ps: Vec<String> = params.iter().map(|(p, t)| format!("{p}: {}", t.name())).collect();
        let n = 2 + self.rng.below(4) as usize;
        let body = self.stmts(&mut scope, 2, ret, false, n);
        let (tail, b) = self.e(ret, &scope, 2, ret);
        let tail = match ret {
            T::Os | T::Rs => format!("{tail}.map(|s: Str| s + out)"),
            T::J => format!(
                "match {tail} {{ J::S(s) => J::S(s + out), J::N(n) => J::N(n + out.len()), J::Null => J::S(out) }}"
            ),
            _ => format!("{tail}.map(|o: Option<Str>| o.map(|s: Str| s + out))"),
        };
        self.helper[i] = b + self.out;
        self.in_helper = false;
        format!(
            "fn {name}({}) -> {} {{\n    let mut out: Str = \"\" + \"\";\n    {}\n    {tail}\n}}\n",
            ps.join(", "),
            ret.name(),
            body.join("\n    ")
        )
    }
}
