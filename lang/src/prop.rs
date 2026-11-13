//! Property tests (T2): a `#[test]` with data parameters runs on generated inputs. This module
//! generates them, shrinks a failing case, shows a value as a literal and builds the entry that
//! runs one case. Deterministic: a seed gives the same values everywhere, from its own generator.

use crate::eval::{Key, Value};
use crate::hmap::HashMap;
use crate::ir::*;
use crate::Span;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The longest generated `Str`, `Bytes`, `List` or map, reached as the case index grows.
pub const MAX_SIZE: usize = 32;

/// Container nesting from which a recursive enum variant is no longer chosen. Each level of
/// nesting also divides a container's length by 4, so at this depth containers are empty.
const MAX_DEPTH: usize = 4;

/// SplitMix64: small, fast and the same on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The generator of case `i` of a run seeded `seed`: each case has its own stream, so a
    /// case's input does not depend on how many cases run.
    pub fn for_case(seed: u64, i: u64) -> Rng {
        let mut r = Rng(seed ^ i.wrapping_mul(0xA076_1D64_78BD_642F));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`, `n > 0`.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// A number in `lo..=hi`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }
}

/// The size of case `i`: it bounds lengths, and grows with the case index up to `MAX_SIZE`.
pub fn size_of_case(i: usize) -> usize {
    i.min(MAX_SIZE)
}

/// A value of `ty` for a case of size `size`: `Int` mixes small values, the boundaries (0, ±1,
/// `i64::MIN`, `i64::MAX`) and wide ones; `Float` finite values and 0.0, -0.0, NaN, ±inf; `Str`
/// printable ASCII with some multi-byte characters; `Bytes` any byte; enum variants uniformly.
pub fn generate(m: &Module, ty: &Ty, rng: &mut Rng, size: usize) -> Value {
    gen(m, ty, rng, size, 0)
}

const INT_BOUNDS: [i64; 5] = [0, 1, -1, i64::MIN, i64::MAX];
const FLOAT_SPECIALS: [f64; 5] = [0.0, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
const MULTI_BYTE: [char; 8] = ['é', 'ß', 'λ', 'ж', '中', '日', '😀', '🦀'];

fn gen(m: &Module, ty: &Ty, r: &mut Rng, size: usize, depth: usize) -> Value {
    // A length at this depth: up to `size` at the top, a quarter of that one level down.
    let cap = size >> (2 * depth).min(63);
    match ty {
        Ty::Int => Value::Int(match r.below(10) {
            0..=3 => r.range(-10, 10),
            4 | 5 => INT_BOUNDS[r.below(INT_BOUNDS.len())],
            6 | 7 => r.range(-1_000_000, 1_000_000),
            _ => r.next_u64() as i64,
        }),
        Ty::Float => Value::Float(match r.below(10) {
            0 | 1 => FLOAT_SPECIALS[r.below(FLOAT_SPECIALS.len())],
            2..=4 => r.range(-10, 10) as f64,
            5..=7 => (r.next_u64() as f64 / u64::MAX as f64 - 0.5) * 2000.0,
            _ => loop {
                let x = f64::from_bits(r.next_u64());
                if x.is_finite() {
                    break x;
                }
            },
        }),
        Ty::Bool => Value::Bool(r.below(2) == 1),
        Ty::Unit => Value::Unit,
        Ty::Str => {
            let n = r.below(cap + 1);
            let char = |r: &mut Rng| match r.below(10) {
                0 => MULTI_BYTE[r.below(MULTI_BYTE.len())],
                _ => char::from(r.range(0x20, 0x7e) as u8),
            };
            Value::Str((0..n).map(|_| char(r)).collect::<String>().into())
        }
        Ty::Bytes => {
            let n = r.below(cap + 1);
            Value::Bytes(Rc::new((0..n).map(|_| r.next_u64() as u8).collect()))
        }
        Ty::List(t) => {
            let n = r.below(cap + 1);
            Value::List(Rc::new((0..n).map(|_| gen(m, t, r, size, depth + 1)).collect()))
        }
        Ty::Map(k, v) => {
            let n = r.below(cap + 1);
            let mut map = BTreeMap::new();
            for _ in 0..n {
                let key = key(gen(m, k, r, size, depth + 1));
                map.insert(key, gen(m, v, r, size, depth + 1));
            }
            Value::Map(Rc::new(map))
        }
        Ty::HashMap(k, v) => {
            let n = r.below(cap + 1);
            let mut map = HashMap::new();
            for _ in 0..n {
                let key = key(gen(m, k, r, size, depth + 1));
                map.insert(key, gen(m, v, r, size, depth + 1));
            }
            Value::HashMap(Rc::new(map))
        }
        Ty::Option(t) => match r.below(2) {
            0 => Value::Enum(EnumRef::Option, 0, vec![]),
            _ => Value::Enum(EnumRef::Option, 1, vec![gen(m, t, r, size, depth)]),
        },
        Ty::Result(t, e) => match r.below(2) {
            0 => Value::Enum(EnumRef::Result, 0, vec![gen(m, t, r, size, depth)]),
            _ => Value::Enum(EnumRef::Result, 1, vec![gen(m, e, r, size, depth)]),
        },
        Ty::Struct(s) => {
            let fields = &m.structs[s.0 as usize].fields;
            Value::Struct(*s, fields.iter().map(|(_, t)| gen(m, t, r, size, depth)).collect())
        }
        Ty::Enum(e) => {
            let variants = &m.enums[e.0 as usize].variants;
            let mut choices: Vec<usize> = (0..variants.len()).collect();
            if depth >= MAX_DEPTH {
                let base: Vec<usize> = (choices.iter().copied())
                    .filter(|&i| !variants[i].1.iter().any(|t| mentions(m, t, *e, &mut vec![])))
                    .collect();
                if !base.is_empty() {
                    choices = base;
                }
            }
            let i = choices[r.below(choices.len())];
            let payload = variants[i].1.iter().map(|t| gen(m, t, r, size, depth)).collect();
            Value::Enum(EnumRef::User(*e), i, payload)
        }
        Ty::Cap(_) | Ty::Fn(..) | Ty::Never => unreachable!("checked: a test parameter is generable"),
    }
}

/// A map key from a generated `Int` or `Str`.
fn key(v: Value) -> Key {
    match v {
        Value::Int(n) => Key::Int(n),
        Value::Str(s) => Key::Str(s.to_string()),
        _ => unreachable!("checked key"),
    }
}

/// Whether `ty` holds enum `e`, through any struct, enum or container; `seen` are the structs
/// and enums already looked into.
fn mentions(m: &Module, ty: &Ty, e: EnumId, seen: &mut Vec<Ty>) -> bool {
    match ty {
        Ty::Enum(x) if *x == e => true,
        Ty::Option(t) | Ty::List(t) => mentions(m, t, e, seen),
        Ty::Result(a, b) | Ty::Map(a, b) | Ty::HashMap(a, b) => mentions(m, a, e, seen) || mentions(m, b, e, seen),
        Ty::Struct(_) | Ty::Enum(_) if seen.contains(ty) => false,
        Ty::Struct(s) => {
            seen.push(ty.clone());
            let fields = &m.structs[s.0 as usize].fields;
            fields.iter().any(|(_, t)| mentions(m, t, e, seen))
        }
        Ty::Enum(x) => {
            seen.push(ty.clone());
            let variants = &m.enums[x.0 as usize].variants;
            variants.iter().flat_map(|(_, ts)| ts).any(|t| mentions(m, t, e, seen))
        }
        _ => false,
    }
}

/// The values one step smaller than `v` of type `ty`, smallest first: `Int` toward 0, `Float`
/// toward 0.0, `Str`, `Bytes`, `List` and maps by dropping elements then shrinking each,
/// structs field by field, enums toward the first variant, then by their payload.
pub fn shrink(m: &Module, ty: &Ty, v: &Value) -> Vec<Value> {
    candidates(m, ty, v.clone()).collect()
}

/// A lazy sequence of smaller values.
type Smaller<'a, T> = Box<dyn Iterator<Item = T> + 'a>;

/// `shrink`, made one candidate at a time: shrinking takes only as many as it runs, so its
/// host work is bounded by its runs times the value's size, not the size squared.
fn candidates<'a>(m: &'a Module, ty: &'a Ty, v: Value) -> Smaller<'a, Value> {
    match (ty, v) {
        (_, Value::Int(n)) => Box::new(shrink_int(n).into_iter().map(Value::Int)),
        (_, Value::Float(x)) => Box::new(shrink_float(x).into_iter().map(Value::Float)),
        (_, Value::Bool(true)) => Box::new(std::iter::once(Value::Bool(false))),
        (_, Value::Str(s)) => {
            let chars: Rc<Vec<char>> = Rc::new(s.chars().collect());
            let each = seq(chars, |c: &char| -> Smaller<'a, char> {
                Box::new((*c != 'a').then_some('a').into_iter())
            });
            Box::new(each.map(|cs| Value::Str(cs.into_iter().collect::<String>().into())))
        }
        (_, Value::Bytes(b)) => {
            let each = seq(b, |b: &u8| -> Smaller<'a, u8> {
                Box::new((*b != 0).then_some(0).into_iter())
            });
            Box::new(each.map(|b| Value::Bytes(Rc::new(b))))
        }
        (Ty::List(t), Value::List(xs)) => {
            let each = seq(xs, move |x: &Value| candidates(m, t, x.clone()));
            Box::new(each.map(|xs| Value::List(Rc::new(xs))))
        }
        (Ty::Map(_, t) | Ty::HashMap(_, t), v @ (Value::Map(_) | Value::HashMap(_))) => {
            let (entries, hashed): (Vec<(Key, Value)>, bool) = match &v {
                Value::Map(map) => (map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(), false),
                Value::HashMap(map) => (map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(), true),
                _ => unreachable!(),
            };
            let smaller = move |(k, v): &(Key, Value)| -> Smaller<'a, (Key, Value)> {
                let k = k.clone();
                Box::new(candidates(m, t, v.clone()).map(move |v| (k.clone(), v)))
            };
            let map = move |es: Vec<(Key, Value)>| match hashed {
                false => Value::Map(Rc::new(es.into_iter().collect())),
                true => {
                    let mut map = HashMap::new();
                    for (k, v) in es {
                        map.insert(k, v);
                    }
                    Value::HashMap(Rc::new(map))
                }
            };
            Box::new(seq(Rc::new(entries), smaller).map(map))
        }
        (Ty::Struct(s), Value::Struct(_, vs)) => {
            let tys: Vec<&'a Ty> = m.structs[s.0 as usize].fields.iter().map(|f| &f.1).collect();
            let s = *s;
            Box::new(parts(m, tys, vs).map(move |vs| Value::Struct(s, vs)))
        }
        (_, Value::Enum(en, i, vs)) => {
            let variants = Rc::new(variants(m, ty));
            let firsts = {
                let variants = variants.clone();
                (0..i).map(move |j| Value::Enum(en, j, variants[j].iter().map(|t| minimal(m, t)).collect()))
            };
            let payload = parts(m, variants[i].clone(), vs).map(move |vs| Value::Enum(en, i, vs));
            Box::new(firsts.chain(payload))
        }
        _ => Box::new(std::iter::empty()),
    }
}

/// The payload types of each variant of the enum `ty`, `Option` and `Result` included.
fn variants<'a>(m: &'a Module, ty: &'a Ty) -> Vec<Vec<&'a Ty>> {
    match ty {
        Ty::Option(t) => vec![vec![], vec![&**t]],
        Ty::Result(t, e) => vec![vec![&**t], vec![&**e]],
        Ty::Enum(e) => m.enums[e.0 as usize]
            .variants
            .iter()
            .map(|v| v.1.iter().collect())
            .collect(),
        _ => unreachable!("an enum value has an enum type"),
    }
}

/// `vs` with one part, of type `tys[i]`, one step smaller: each part in order.
fn parts<'a>(m: &'a Module, tys: Vec<&'a Ty>, vs: Vec<Value>) -> Smaller<'a, Vec<Value>> {
    let vs = Rc::new(vs);
    Box::new((0..vs.len()).flat_map(move |i| {
        let vs = vs.clone();
        candidates(m, tys[i], vs[i].clone()).map(move |c| {
            let mut next = vs.to_vec();
            next[i] = c;
            next
        })
    }))
}

/// `xs` shorter (empty, either half, then each element dropped), then with one element one
/// step smaller.
fn seq<'a, T: Clone + 'a>(xs: Rc<Vec<T>>, smaller: impl Fn(&T) -> Smaller<'a, T> + 'a) -> Smaller<'a, Vec<T>> {
    let n = xs.len();
    if n == 0 {
        return Box::new(std::iter::empty());
    }
    let halves = {
        let xs = xs.clone();
        (0..if n >= 4 { 2 } else { 0 }).map(move |h| match h {
            0 => xs[n / 2..].to_vec(),
            _ => xs[..n / 2].to_vec(),
        })
    };
    let drops = {
        let xs = xs.clone();
        (0..if n >= 2 { n } else { 0 }).map(move |i| {
            let mut ys = xs.to_vec();
            ys.remove(i);
            ys
        })
    };
    let each = (0..n).flat_map(move |i| {
        let xs = xs.clone();
        smaller(&xs[i]).map(move |c| {
            let mut ys = xs.to_vec();
            ys[i] = c;
            ys
        })
    });
    Box::new(std::iter::once(Vec::new()).chain(halves).chain(drops).chain(each))
}

/// 0, then `n` minus each of `n / 2`, `n / 4`, ... 1: halfway to 0, then ever closer to `n`.
/// A negative `n` tries `-n` after 0. Never `n` itself.
fn shrink_int(n: i64) -> Vec<i64> {
    let mut out = vec![0];
    if n < 0 && n != i64::MIN {
        out.push(-n);
    }
    let mut d = n / 2;
    while d != 0 {
        out.push(n - d);
        d /= 2;
    }
    out.dedup();
    out.retain(|&c| c != n);
    out
}

/// 0.0, then (for a finite value) its integer part and its half.
fn shrink_float(x: f64) -> Vec<f64> {
    let mut out = vec![0.0];
    if x.is_finite() {
        out.extend([x.trunc(), x / 2.0]);
    }
    out.retain(|y| y.to_bits() != x.to_bits());
    out.dedup_by(|a, b| a.to_bits() == b.to_bits());
    out
}

/// The smallest value of `ty`: 0, empty, `false`, `None`, the first variant.
fn minimal(m: &Module, ty: &Ty) -> Value {
    match ty {
        Ty::Int => Value::Int(0),
        Ty::Float => Value::Float(0.0),
        Ty::Bool => Value::Bool(false),
        Ty::Str => Value::Str("".into()),
        Ty::Bytes => Value::Bytes(Rc::default()),
        Ty::List(_) => Value::List(Rc::default()),
        Ty::Map(..) => Value::Map(Rc::default()),
        Ty::HashMap(..) => Value::HashMap(Rc::default()),
        Ty::Option(_) => Value::Enum(EnumRef::Option, 0, vec![]),
        Ty::Result(t, _) => Value::Enum(EnumRef::Result, 0, vec![minimal(m, t)]),
        Ty::Struct(s) => {
            let fields = &m.structs[s.0 as usize].fields;
            Value::Struct(*s, fields.iter().map(|(_, t)| minimal(m, t)).collect())
        }
        // Recursion goes through a container, which is empty here, so this ends.
        Ty::Enum(e) => {
            let payload = &m.enums[e.0 as usize].variants[0].1;
            Value::Enum(EnumRef::User(*e), 0, payload.iter().map(|t| minimal(m, t)).collect())
        }
        _ => Value::Unit,
    }
}

/// Shrinks `case` (one value per type in `tys`), on which `fails` is true, until no one-step
/// smaller case fails: each parameter in turn, taking the first smaller value that still fails
/// and starting over from it. Calls `fails` at most `max` times. Returns the case and the calls.
pub fn minimize(
    m: &Module,
    tys: &[Ty],
    mut case: Vec<Value>,
    max: usize,
    mut fails: impl FnMut(&[Value]) -> bool,
) -> (Vec<Value>, usize) {
    let mut runs = 0;
    'smaller: loop {
        for i in 0..case.len() {
            for c in candidates(m, &tys[i], case[i].clone()) {
                if runs == max {
                    break 'smaller;
                }
                let mut next = case.clone();
                next[i] = c;
                runs += 1;
                if fails(&next) {
                    case = next;
                    continue 'smaller;
                }
            }
        }
        break;
    }
    (case, runs)
}

/// `v` as a kio literal, with the names declared in `m`: `Order { id: "a", total: 3 }`,
/// `Some(1)`, `[1, 2]`, `"hi".to_bytes()`, `0.0 / 0.0` for NaN. A map, which has no literal,
/// shows as `kio run` prints it: `{1: "a"}`.
pub fn debug_literal(m: &Module, v: &Value) -> String {
    let list = |vs: &[Value]| vs.iter().map(|v| debug_literal(m, v)).collect::<Vec<_>>().join(", ");
    let entries = |es: Vec<(&Key, &Value)>| {
        let es: Vec<String> = (es.into_iter())
            .map(|(k, v)| format!("{}: {}", debug_literal(m, &k.to_value()), debug_literal(m, v)))
            .collect();
        format!("{{{}}}", es.join(", "))
    };
    match v {
        Value::Int(i64::MIN) => "-9223372036854775807 - 1".to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(x) => float_literal(*x),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => str_literal(s),
        Value::Bytes(b) => match std::str::from_utf8(b) {
            Ok(s) if !s.chars().any(|c| c.is_control()) => format!("{}.to_bytes()", str_literal(s)),
            _ => format!("\"{}\".from_hex().unwrap_or(\"\".to_bytes())", crate::digest::to_hex(b)),
        },
        Value::Unit => "()".to_string(),
        Value::List(vs) => format!("[{}]", list(vs)),
        Value::Map(map) => entries(map.iter().collect()),
        Value::HashMap(map) => entries(map.iter().collect()),
        Value::Struct(s, vs) => {
            let s = &m.structs[s.0 as usize];
            let fields: Vec<_> = (s.fields.iter().zip(vs))
                .map(|((f, _), v)| format!("{f}: {}", debug_literal(m, v)))
                .collect();
            match fields.is_empty() {
                true => format!("{} {{}}", s.name),
                false => format!("{} {{ {} }}", s.name, fields.join(", ")),
            }
        }
        Value::Enum(en, i, vs) => {
            let variant = match en {
                EnumRef::User(e) => {
                    let e = &m.enums[e.0 as usize];
                    format!("{}::{}", e.name, e.variants[*i].0)
                }
                EnumRef::Option => ["None", "Some"][*i].to_string(),
                EnumRef::Result => ["Ok", "Err"][*i].to_string(),
            };
            match vs.is_empty() {
                true => variant,
                false => format!("{variant}({})", list(vs)),
            }
        }
        _ => crate::eval::debug(m, v),
    }
}

/// A `Float` as a literal kio reads: `1.0`, `-0.0`, `1.0e300`; NaN and ±inf as divisions.
fn float_literal(x: f64) -> String {
    if x.is_nan() {
        return "0.0 / 0.0".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "1.0 / 0.0" } else { "-1.0 / 0.0" }.to_string();
    }
    let s = format!("{x:?}");
    match s.find('e') {
        Some(e) if !s[..e].contains('.') => format!("{}.0{}", &s[..e], &s[e..]),
        _ => s,
    }
}

/// `s` quoted with kio's escapes.
fn str_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Name and type of each data parameter of `m`'s entry, a test: a property's inputs.
pub fn inputs(m: &Module) -> Vec<(String, Ty)> {
    let f = &m.funcs[m.main.0 as usize];
    (f.locals[..f.params as usize].iter())
        .filter(|l| !matches!(l.ty, Ty::Cap(_)))
        .map(|l| (l.name.clone(), l.ty.clone()))
        .collect()
}

/// Makes `m`'s entry, a test with data parameters, a function that a new entry calls with
/// `args` (one per data parameter, in order) and its own capability parameters, which the new
/// entry takes by the same names: a case runs as any `main` does, interpreted or compiled.
pub fn case_entry(m: &mut Module, args: &[Value]) {
    let test = m.main;
    let f = &m.funcs[test.0 as usize];
    let span = f.span;
    let params = &f.locals[..f.params as usize];
    let mut locals: Vec<Local> = params.iter().filter(|l| matches!(l.ty, Ty::Cap(_))).cloned().collect();
    let caps = locals.len() as u32;
    let mut b = Build {
        locals: &mut locals,
        stmts: Vec::new(),
        span,
    };
    let (mut cap, mut arg) = (0, args.iter());
    let mut call = Vec::new();
    for p in params {
        call.push(match p.ty {
            Ty::Cap(_) => {
                cap += 1;
                Expr {
                    kind: ExprKind::Local(LocalId(cap - 1)),
                    ty: p.ty.clone(),
                    span,
                }
            }
            _ => b.literal(m, arg.next().expect("one value per data parameter"), &p.ty),
        });
    }
    let ret = f.ret.clone();
    let body = Block {
        stmts: b.stmts,
        tail: Some(Box::new(Expr {
            kind: ExprKind::Call(test, call),
            ty: ret.clone(),
            span,
        })),
    };
    let name = format!("{}::case", f.name);
    m.funcs.push(Func {
        name,
        locals,
        params: caps,
        ret,
        body,
        span,
    });
    m.main = FuncId(m.funcs.len() as u32 - 1);
}

/// The statements and locals that build a case's values.
struct Build<'a> {
    locals: &'a mut Vec<Local>,
    stmts: Vec<Stmt>,
    span: Span,
}

impl Build<'_> {
    fn expr(&self, kind: ExprKind, ty: Ty) -> Expr {
        Expr {
            kind,
            ty,
            span: self.span,
        }
    }

    /// A new local of type `ty`, set to `init`.
    fn local(&mut self, ty: &Ty, init: Expr) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(Local {
            name: format!("case#{}", id.0),
            ty: ty.clone(),
            span: self.span,
        });
        let kind = StmtKind::Let(id, init);
        self.stmts.push(Stmt { kind, span: self.span });
        id
    }

    /// `op(args)` on local `id`, as a statement.
    fn mutate(&mut self, id: LocalId, op: Mut, args: Vec<Expr>) {
        let place = Place {
            root: id,
            steps: vec![],
        };
        let e = self.expr(ExprKind::Mutate(place, op, args), Ty::Unit);
        self.stmts.push(Stmt {
            kind: StmtKind::Expr(e),
            span: self.span,
        });
    }

    /// An expression with value `v` of type `ty`; a `Bytes` or map is built in a local first.
    fn literal(&mut self, m: &Module, v: &Value, ty: &Ty) -> Expr {
        let kind = match (ty, v) {
            (_, Value::Int(n)) => ExprKind::Int(*n),
            (_, Value::Float(x)) => ExprKind::Float(*x),
            (_, Value::Bool(b)) => ExprKind::Bool(*b),
            (_, Value::Str(s)) => ExprKind::Str(s.clone()),
            (_, Value::Unit) => ExprKind::Unit,
            (_, Value::Bytes(bs)) => {
                let empty = self.expr(ExprKind::Str("".into()), Ty::Str);
                let init = self.expr(
                    ExprKind::Builtin(Builtin::StrToBytes, Box::new(empty), vec![]),
                    Ty::Bytes,
                );
                let id = self.local(ty, init);
                for b in bs.iter() {
                    let b = self.expr(ExprKind::Int(i64::from(*b)), Ty::Int);
                    self.mutate(id, Mut::BytesPush, vec![b]);
                }
                ExprKind::Local(id)
            }
            (Ty::List(t), Value::List(vs)) => ExprKind::List(vs.iter().map(|v| self.literal(m, v, t)).collect()),
            (Ty::Map(k, t) | Ty::HashMap(k, t), Value::Map(_) | Value::HashMap(_)) => {
                let entries: Vec<(Value, Value)> = match v {
                    Value::Map(map) => map.iter().map(|(k, v)| (k.to_value(), v.clone())).collect(),
                    Value::HashMap(map) => map.iter().map(|(k, v)| (k.to_value(), v.clone())).collect(),
                    _ => unreachable!(),
                };
                let init = self.expr(ExprKind::MapNew, ty.clone());
                let id = self.local(ty, init);
                for (key, val) in entries {
                    let args = vec![self.literal(m, &key, k), self.literal(m, &val, t)];
                    self.mutate(id, Mut::MapInsert, args);
                }
                ExprKind::Local(id)
            }
            (Ty::Struct(s), Value::Struct(_, vs)) => {
                let fields = &m.structs[s.0 as usize].fields;
                let inits = (fields.iter().zip(vs).enumerate())
                    .map(|(i, ((_, t), v))| (i as u32, self.literal(m, v, t)))
                    .collect();
                ExprKind::StructLit(*s, inits)
            }
            (_, Value::Enum(en, i, vs)) => {
                let tys = variants(m, ty);
                let payload = tys[*i].iter().zip(vs).map(|(t, v)| self.literal(m, v, t)).collect();
                ExprKind::Variant(*en, *i as u32, payload)
            }
            _ => unreachable!("a generated value of its type"),
        };
        self.expr(kind, ty.clone())
    }
}

/// A case's values with nothing shared, so they can move to the thread that runs the case.
#[derive(Debug, Clone, PartialEq)]
pub struct Case(Vec<Owned>);

#[derive(Debug, Clone, PartialEq)]
enum Owned {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    Unit,
    List(Vec<Owned>),
    Map(Vec<(Key, Owned)>),
    HashMap(Vec<(Key, Owned)>),
    Struct(StructId, Vec<Owned>),
    Enum(EnumRef, usize, Vec<Owned>),
}

impl Case {
    /// `values`, generated or shrunk: data only, no function or capability.
    pub fn new(values: &[Value]) -> Case {
        Case(values.iter().map(owned).collect())
    }

    pub fn values(&self) -> Vec<Value> {
        self.0.iter().map(shared).collect()
    }
}

fn owned(v: &Value) -> Owned {
    let each = |vs: &[Value]| vs.iter().map(owned).collect();
    match v {
        Value::Int(n) => Owned::Int(*n),
        Value::Float(x) => Owned::Float(*x),
        Value::Bool(b) => Owned::Bool(*b),
        Value::Str(s) => Owned::Str(s.to_string()),
        Value::Bytes(b) => Owned::Bytes(b.to_vec()),
        Value::Unit => Owned::Unit,
        Value::List(vs) => Owned::List(each(vs)),
        Value::Map(m) => Owned::Map(m.iter().map(|(k, v)| (k.clone(), owned(v))).collect()),
        Value::HashMap(m) => Owned::HashMap(m.iter().map(|(k, v)| (k.clone(), owned(v))).collect()),
        Value::Struct(s, vs) => Owned::Struct(*s, each(vs)),
        Value::Enum(e, i, vs) => Owned::Enum(*e, *i, each(vs)),
        Value::Cap(_) | Value::Fn(_) | Value::Closure(..) => unreachable!("a case holds data only"),
    }
}

fn shared(o: &Owned) -> Value {
    let each = |os: &[Owned]| os.iter().map(shared).collect::<Vec<_>>();
    match o {
        Owned::Int(n) => Value::Int(*n),
        Owned::Float(x) => Value::Float(*x),
        Owned::Bool(b) => Value::Bool(*b),
        Owned::Str(s) => Value::Str(s.as_str().into()),
        Owned::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        Owned::Unit => Value::Unit,
        Owned::List(os) => Value::List(Rc::new(each(os))),
        Owned::Map(es) => Value::Map(Rc::new(es.iter().map(|(k, o)| (k.clone(), shared(o))).collect())),
        Owned::HashMap(es) => {
            let mut map = HashMap::new();
            for (k, o) in es {
                map.insert(k.clone(), shared(o));
            }
            Value::HashMap(Rc::new(map))
        }
        Owned::Struct(s, os) => Value::Struct(*s, each(os)),
        Owned::Enum(e, i, os) => Value::Enum(*e, *i, each(os)),
    }
}
