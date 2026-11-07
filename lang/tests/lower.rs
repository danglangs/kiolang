use lang::ir::*;
use lang::lower::lower;
use lang::{check, check_files, CapTable};

mod common;
use common::corpus;

/// The lowered program as text: each function with its slots, one statement per line. A
/// local is `name#slot`, a field or variant also shows its index.
fn print(m: &Module) -> String {
    let mut out = String::new();
    for f in &m.funcs {
        let p = Printer { m, f };
        let locals: Vec<_> = (f.locals.iter().enumerate())
            .map(|(i, l)| format!("{}#{i}: {}", l.name, p.ty(&l.ty)))
            .collect();
        let params = locals[..f.params as usize].join(", ");
        out += &format!("fn {}({params}) -> {} ", f.name, p.ty(&f.ret));
        p.block(&mut out, &f.body, 0);
        out += "\n";
        if locals.len() > f.params as usize {
            out += &format!("  locals {}\n", locals[f.params as usize..].join(", "));
        }
    }
    out
}

struct Printer<'a> {
    m: &'a Module,
    f: &'a Func,
}

impl Printer<'_> {
    fn ty(&self, t: &Ty) -> String {
        match t {
            Ty::Struct(s) => self.m.structs[s.0 as usize].name.clone(),
            Ty::Enum(e) => self.m.enums[e.0 as usize].name.clone(),
            Ty::Cap(c) => self.m.caps[c.0 as usize].name.clone(),
            Ty::Option(t) => format!("Option<{}>", self.ty(t)),
            Ty::Result(t, e) => format!("Result<{}, {}>", self.ty(t), self.ty(e)),
            Ty::List(t) => format!("List<{}>", self.ty(t)),
            Ty::Map(k, v) => format!("Map<{}, {}>", self.ty(k), self.ty(v)),
            t => format!("{t:?}"),
        }
    }

    fn local(&self, id: LocalId) -> String {
        format!("{}#{}", self.f.locals[id.0 as usize].name, id.0)
    }

    /// The name of variant `i` of a value of type `t`.
    fn variant(&self, t: &Ty, i: u32) -> String {
        let name = match t {
            Ty::Option(_) => ["None", "Some"][i as usize].to_string(),
            Ty::Result(..) => ["Ok", "Err"][i as usize].to_string(),
            Ty::Enum(e) => {
                let def = &self.m.enums[e.0 as usize];
                format!("{}::{}", def.name, def.variants[i as usize].0)
            }
            _ => unreachable!(),
        };
        format!("{name}[{i}]")
    }

    fn block(&self, out: &mut String, b: &Block, depth: usize) {
        let pad = "  ".repeat(depth + 1);
        *out += "{\n";
        for s in &b.stmts {
            *out += &pad;
            self.stmt(out, s, depth + 1);
            *out += "\n";
        }
        if let Some(t) = &b.tail {
            *out += &pad;
            self.expr(out, t, depth + 1);
            *out += "\n";
        }
        *out += &"  ".repeat(depth);
        *out += "}";
    }

    fn stmt(&self, out: &mut String, s: &Stmt, depth: usize) {
        match &s.kind {
            StmtKind::Let(x, e) => {
                *out += &format!("let {} = ", self.local(*x));
                self.expr(out, e, depth);
            }
            StmtKind::Assign(place, op, e) => {
                self.place(out, place, depth);
                *out += &match op {
                    Some(op) => format!(" {op:?}= "),
                    None => " = ".to_string(),
                };
                self.expr(out, e, depth);
            }
            StmtKind::Return(e) => {
                *out += "return";
                if let Some(e) = e {
                    *out += " ";
                    self.expr(out, e, depth);
                }
            }
            StmtKind::While(c, b) => {
                *out += "while ";
                self.expr(out, c, depth);
                *out += " ";
                self.block(out, b, depth);
            }
            StmtKind::Loop(b) => {
                *out += "loop ";
                self.block(out, b, depth);
            }
            StmtKind::ForList(x, xs, b) => {
                *out += &format!("for {} in ", self.local(*x));
                self.expr(out, xs, depth);
                *out += " ";
                self.block(out, b, depth);
            }
            StmtKind::ForRange(x, a, z, b) => {
                *out += &format!("for {} in ", self.local(*x));
                self.expr(out, a, depth);
                *out += "..";
                self.expr(out, z, depth);
                *out += " ";
                self.block(out, b, depth);
            }
            StmtKind::Break => *out += "break",
            StmtKind::Continue => *out += "continue",
            StmtKind::Expr(e) => self.expr(out, e, depth),
        }
    }

    fn place(&self, out: &mut String, p: &Place, depth: usize) {
        *out += &self.local(p.root);
        let mut ty = &self.f.locals[p.root.0 as usize].ty;
        for step in &p.steps {
            match (step, ty) {
                (PlaceStep::Field(i), Ty::Struct(s)) => {
                    let (name, t) = &self.m.structs[s.0 as usize].fields[*i as usize];
                    *out += &format!(".{name}[{i}]");
                    ty = t;
                }
                (PlaceStep::Index(e, _), _) => {
                    *out += "[";
                    self.expr(out, e, depth);
                    *out += "]";
                    if let Ty::List(t) = ty {
                        ty = t;
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    fn list(&self, out: &mut String, es: &[Expr], depth: usize) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                *out += ", ";
            }
            self.expr(out, e, depth);
        }
    }

    fn expr(&self, out: &mut String, e: &Expr, depth: usize) {
        match &e.kind {
            ExprKind::Int(n) => *out += &n.to_string(),
            ExprKind::Bool(b) => *out += &b.to_string(),
            ExprKind::Str(s) => *out += &format!("{s:?}"),
            ExprKind::Unit => *out += "()",
            ExprKind::Local(x) => *out += &self.local(*x),
            ExprKind::Unary(op, x) => {
                *out += &format!("{op:?} ");
                self.expr(out, x, depth);
            }
            ExprKind::Binary(op, a, b) => {
                *out += "(";
                self.expr(out, a, depth);
                *out += &format!(" {op:?} ");
                self.expr(out, b, depth);
                *out += ")";
            }
            ExprKind::If(c, then, els) => {
                *out += "if ";
                self.expr(out, c, depth);
                *out += " ";
                self.block(out, then, depth);
                if let Some(els) = els {
                    *out += " else ";
                    self.expr(out, els, depth);
                }
            }
            ExprKind::Block(b) => self.block(out, b, depth),
            ExprKind::Call(f, args) => {
                *out += &format!("{}(", self.m.funcs[f.0 as usize].name);
                self.list(out, args, depth);
                *out += ")";
            }
            ExprKind::CapCall(x, i, args) => {
                let Ty::Cap(c) = &self.f.locals[x.0 as usize].ty else {
                    unreachable!()
                };
                let method = &self.m.caps[c.0 as usize].methods[*i as usize].0;
                *out += &format!("{}.{method}[{i}](", self.local(*x));
                self.list(out, args, depth);
                *out += ")";
            }
            ExprKind::Builtin(b, recv, args) => {
                self.expr(out, recv, depth);
                *out += &format!(".{b:?}(");
                self.list(out, args, depth);
                *out += ")";
            }
            ExprKind::Mutate(place, m, args) => {
                self.place(out, place, depth);
                *out += &format!(".{m:?}(");
                self.list(out, args, depth);
                *out += ")";
            }
            ExprKind::List(items) => {
                *out += "[";
                self.list(out, items, depth);
                *out += "]";
            }
            ExprKind::MapNew => *out += "Map::new()",
            ExprKind::StructLit(s, inits) => {
                let def = &self.m.structs[s.0 as usize];
                *out += &format!("{} {{ ", def.name);
                for (i, x) in inits {
                    *out += &format!("{}[{i}]: ", def.fields[*i as usize].0);
                    self.expr(out, x, depth);
                    *out += ", ";
                }
                *out += "}";
            }
            ExprKind::Field(base, i) => {
                self.expr(out, base, depth);
                let Ty::Struct(s) = &base.ty else { unreachable!() };
                *out += &format!(".{}[{i}]", self.m.structs[s.0 as usize].fields[*i as usize].0);
            }
            ExprKind::Index(base, i) => {
                self.expr(out, base, depth);
                *out += "[";
                self.expr(out, i, depth);
                *out += "]";
            }
            ExprKind::Variant(_, i, args) => {
                *out += &self.variant(&e.ty, *i);
                if !args.is_empty() {
                    *out += "(";
                    self.list(out, args, depth);
                    *out += ")";
                }
            }
            ExprKind::Match(scrut, arms) => {
                *out += "match ";
                self.expr(out, scrut, depth);
                *out += " {\n";
                for arm in arms {
                    *out += &"  ".repeat(depth + 1);
                    *out += &match &arm.pat {
                        Pat::Wild => "_".to_string(),
                        Pat::Bind(x) => self.local(*x),
                        Pat::Int(n) => n.to_string(),
                        Pat::Str(s) => format!("{s:?}"),
                        Pat::Bool(b) => b.to_string(),
                        Pat::Variant(i, binds) => {
                            let binds: Vec<_> = (binds.iter())
                                .map(|b| b.map_or("_".to_string(), |x| self.local(x)))
                                .collect();
                            let binds = if binds.is_empty() {
                                String::new()
                            } else {
                                format!("({})", binds.join(", "))
                            };
                            format!("{}{binds}", self.variant(&scrut.ty, *i))
                        }
                    };
                    *out += " => ";
                    self.expr(out, &arm.body, depth + 1);
                    *out += "\n";
                }
                *out += &"  ".repeat(depth);
                *out += "}";
            }
            ExprKind::Try(x) => {
                self.expr(out, x, depth);
                *out += "?";
            }
        }
    }
}

/// Compares the lowered `src` line by line, ignoring the indentation of `want`, which is
/// written inside a test. A `src` without `main` gets an empty one, left out of the output.
fn snapshot(src: &str, want: &str) {
    const MAIN: &str = "fn main() {}";
    let src = if src.contains("fn main(") {
        src.to_string()
    } else {
        format!("{src} {MAIN}")
    };
    let caps = CapTable::v0();
    let p = check(&src, &caps).unwrap_or_else(|d| panic!("{d:?}"));
    let got = print(&lower(&p, &caps)).replace("fn main() -> Unit {\n}\n", "");
    let want: Vec<_> = want.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let got_trimmed: Vec<_> = got.lines().map(str::trim).collect();
    assert_eq!(got_trimmed, want, "\n{got}");
}

#[test]
fn locals_and_shadowing() {
    snapshot(
        "fn f(n: Int) -> Int { let x = n; let x = x + 1; { let x = 2; x; } x }",
        "fn f(n#0: Int) -> Int {
           let x#1 = n#0
           let x#2 = (x#1 Add 1)
           {
             let x#3 = 2
             x#3
           }
           x#2
         }
         locals x#1: Int, x#2: Int, x#3: Int",
    );
}

#[test]
fn calls_across_modules() {
    let caps = CapTable::v0();
    let files = [
        (
            "app",
            "mod web; fn main() -> Int { web::inc(two()) } fn two() -> Int { 2 }",
        ),
        (
            "web",
            "pub fn inc(n: Int) -> Int { add(n, 1) } fn add(a: Int, b: Int) -> Int { a + b }",
        ),
    ];
    let p = check_files(&files, &caps).unwrap_or_else(|d| panic!("{d:?}"));
    let got = print(&lower(&p, &caps));
    let want = "fn main() -> Int {\n  web::inc(two())\n}
fn two() -> Int {\n  2\n}
fn web::add(a#0: Int, b#1: Int) -> Int {\n  (a#0 Add b#1)\n}
fn web::inc(n#0: Int) -> Int {\n  web::add(n#0, 1)\n}\n";
    assert_eq!(got, want);
}

#[test]
fn each_operator() {
    snapshot(
        "fn f(a: Int, s: Str, p: Bool) -> Bool {
            let n = -(a + a - a * a / a % a);
            let t = s + s;
            a < a && a <= a || a > a && a >= a || a == a && s != t || !p
        }",
        "fn f(a#0: Int, s#1: Str, p#2: Bool) -> Bool {
           let n#3 = Neg ((a#0 Add a#0) Sub (((a#0 Mul a#0) Div a#0) Rem a#0))
           let t#4 = (s#1 Concat s#1)
           (((((a#0 Lt a#0) And (a#0 Le a#0)) Or ((a#0 Gt a#0) And (a#0 Ge a#0))) Or ((a#0 Eq a#0) And (s#1 Ne t#4))) Or Not p#2)
         }
         locals n#3: Int, t#4: Str",
    );
}

#[test]
fn compound_assignment_names_its_operator() {
    snapshot(
        "fn main() { let mut n = 1; n += 2; n -= 1; let mut s = \"a\"; s += \"b\"; }",
        "fn main() -> Unit {
           let n#0 = 1
           n#0 Add= 2
           n#0 Sub= 1
           let s#1 = \"a\"
           s#1 Concat= \"b\"
         }
         locals n#0: Int, s#1: Str",
    );
}

#[test]
fn mutation_in_a_match_arm() {
    snapshot(
        "fn f(o: Option<Int>) -> List<Int> {
            let mut xs: List<Int> = [];
            match o { Some(x) => xs.push(x), None => {} }
            xs
        }",
        "fn f(o#0: Option<Int>) -> List<Int> {
           let xs#1 = []
           match o#0 {
             Some[1](x#2) => xs#1.ListPush(x#2)
             None[0] => {
             }
           }
           xs#1
         }
         locals xs#1: List<Int>, x#2: Int",
    );
}

#[test]
fn struct_literal_keeps_source_order() {
    snapshot(
        "struct P { x: Int, y: Int } fn main() -> Int { let p = P { y: 1, x: 2 }; p.y }",
        "fn main() -> Int {
           let p#0 = P { y[1]: 1, x[0]: 2, }
           p#0.y[1]
         }
         locals p#0: P",
    );
}

#[test]
fn nested_places() {
    snapshot(
        "struct C { c: Int } struct B { b: List<C> } struct A { a: B }
         fn f(i: Int) {
            let mut x = A { a: B { b: [C { c: 1 }] } };
            x.a.b[i].c = 2;
            x.a.b[i + 1].c += 3;
            let mut m: Map<Str, List<Bytes>> = Map::new();
            m.insert(\"k\", []);
            m.remove(\"k\");
            let mut bs = [\"a\".to_bytes()];
            bs[0].push(i);
         }",
        "fn f(i#0: Int) -> Unit {
           let x#1 = A { a[0]: B { b[0]: [C { c[0]: 1, }], }, }
           x#1.a[0].b[0][i#0].c[0] = 2
           x#1.a[0].b[0][(i#0 Add 1)].c[0] Add= 3
           let m#2 = Map::new()
           m#2.MapInsert(\"k\", [])
           m#2.MapRemove(\"k\")
           let bs#3 = [\"a\".StrToBytes()]
           bs#3[0].BytesPush(i#0)
         }
         locals x#1: A, m#2: Map<Str, List<Bytes>>, bs#3: List<Bytes>",
    );
}

#[test]
fn try_and_matches() {
    snapshot(
        "enum Shape { Dot, Square(Int), Rect(Int, Int) }
         fn area(s: Shape) -> Int { match s { Shape::Dot => 0, Square(n) => n * n, Shape::Rect(w, _) => w } }
         fn first(xs: List<Str>) -> Option<Int> { let n = xs[0].parse_int()?; Some(n) }
         fn name(n: Int) -> Str { match n { 0 => \"zero\", other => other.to_str() } }
         fn main() -> Int { match \"a\" { \"a\" => area(Shape::Rect(2, 3)), _ => area(Shape::Dot) } }",
        "fn area(s#0: Shape) -> Int {
           match s#0 {
             Shape::Dot[0] => 0
             Shape::Square[1](n#1) => (n#1 Mul n#1)
             Shape::Rect[2](w#2, _) => w#2
           }
         }
         locals n#1: Int, w#2: Int
         fn first(xs#0: List<Str>) -> Option<Int> {
           let n#1 = xs#0[0].StrParseInt()?
           Some[1](n#1)
         }
         locals n#1: Int
         fn main() -> Int {
           match \"a\" {
             \"a\" => area(Shape::Rect[2](2, 3))
             _ => area(Shape::Dot[0])
           }
         }
         fn name(n#0: Int) -> Str {
           match n#0 {
             0 => \"zero\"
             other#1 => other#1.IntToStr()
           }
         }
         locals other#1: Int",
    );
}

#[test]
fn loops() {
    snapshot(
        "fn f(xs: List<Int>) -> Int {
            let mut t = 0;
            for x in xs { t += x; }
            for i in 0..xs.len() { if i == 2 { continue; } t += i; }
            loop { break; }
            while t > 0 { t -= 1; }
            t
        }",
        "fn f(xs#0: List<Int>) -> Int {
           let t#1 = 0
           for x#2 in xs#0 {
             t#1 Add= x#2
           }
           for i#3 in 0..xs#0.ListLen() {
             if (i#3 Eq 2) {
               continue
             }
             t#1 Add= i#3
           }
           loop {
             break
           }
           while (t#1 Gt 0) {
             t#1 Sub= 1
           }
           t#1
         }
         locals t#1: Int, x#2: Int, i#3: Int",
    );
}

#[test]
fn capability_calls() {
    snapshot(
        "fn load(d: Dir) -> Result<Bytes, IoErr> { d.read(\"a\") }
         fn main(input: Dir, out: Dir) -> Result<(), IoErr> {
            let b = load(input)?;
            match out.create(\"b\", b) { Ok(_) => Ok(()), Err(e) => Err(e) }
         }",
        "fn load(d#0: Dir) -> Result<Bytes, IoErr> {
           d#0.read[0](\"a\")
         }
         fn main(input#0: Dir, out#1: Dir) -> Result<Unit, IoErr> {
           let b#2 = load(input#0)?
           match out#1.create[1](\"b\", b#2) {
             Ok[0](_) => Ok[0](())
             Err[1](e#3) => Err[1](e#3)
           }
         }
         locals b#2: Bytes, e#3: IoErr",
    );
}

#[test]
fn capability_error_enums_come_first() {
    let caps = CapTable::v0();
    let p = check("enum A { X } fn main() {}", &caps).unwrap();
    let m = lower(&p, &caps);
    let names: Vec<_> = m.enums.iter().map(|e| e.name.as_str()).collect();
    let mut want: Vec<_> = caps.enums.iter().map(|e| e.name.as_str()).collect();
    want.push("A");
    assert_eq!(names, want);
    let caps_names: Vec<_> = m.caps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        caps_names,
        caps.caps.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(m.funcs[m.main.0 as usize].name, "main");
}

/// `!` inside a type, from an expression that never produces its value, becomes `()`.
#[test]
fn diverging_payload() {
    snapshot(
        "fn f() -> Option<Int> { let x = Some({ return None; }); None }",
        "fn f() -> Option<Int> {
           let x#0 = Some[1]({
             return None[0]
           })
           None[0]
         }
         locals x#0: Option<Unit>",
    );
}

/// Each type implied by an expression's parts agrees with its `ty`, so slots, fields and
/// variants were resolved to the right ids.
struct Types<'a> {
    m: &'a Module,
    f: &'a Func,
}

impl Types<'_> {
    fn local(&self, x: LocalId) -> &Ty {
        &self.f.locals[x.0 as usize].ty
    }

    fn agree(&self, ty: &Ty, want: &Ty, what: &str) {
        assert!(
            *ty == Ty::Never || ty == want,
            "{}: {what}: {ty:?} vs {want:?}",
            self.f.name
        );
    }

    fn block(&self, b: &Block) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let(x, e) => {
                    self.expr(e);
                    if !matches!(e.ty, Ty::Never) {
                        self.agree(&e.ty, self.local(*x), "let");
                    }
                }
                StmtKind::Assign(p, _, e) => {
                    let t = self.place(p);
                    self.expr(e);
                    self.agree(&e.ty, &t, "assign");
                }
                StmtKind::Return(e) => {
                    if let Some(e) = e {
                        self.expr(e);
                        self.agree(&e.ty, &self.f.ret, "return");
                    }
                }
                StmtKind::While(c, b) => {
                    self.expr(c);
                    self.agree(&c.ty, &Ty::Bool, "while");
                    self.block(b);
                }
                StmtKind::Loop(b) => self.block(b),
                StmtKind::ForList(x, xs, b) => {
                    self.expr(xs);
                    self.agree(&xs.ty, &Ty::List(Box::new(self.local(*x).clone())), "for");
                    self.block(b);
                }
                StmtKind::ForRange(x, a, z, b) => {
                    self.expr(a);
                    self.expr(z);
                    self.agree(self.local(*x), &Ty::Int, "range");
                    self.block(b);
                }
                StmtKind::Break | StmtKind::Continue => {}
                StmtKind::Expr(e) => self.expr(e),
            }
        }
        if let Some(t) = &b.tail {
            self.expr(t);
        }
    }

    fn place(&self, p: &Place) -> Ty {
        let mut ty = self.local(p.root).clone();
        for step in &p.steps {
            ty = match (step, ty) {
                (PlaceStep::Field(i), Ty::Struct(s)) => self.m.structs[s.0 as usize].fields[*i as usize].1.clone(),
                (PlaceStep::Index(e, _), t) => {
                    self.expr(e);
                    self.agree(&e.ty, &Ty::Int, "index");
                    match t {
                        Ty::List(t) => *t,
                        Ty::Bytes => Ty::Int,
                        t => panic!("index into {t:?}"),
                    }
                }
                (_, t) => panic!("field of {t:?}"),
            };
        }
        ty
    }

    fn expr(&self, e: &Expr) {
        let want = match &e.kind {
            ExprKind::Int(_) => Some(Ty::Int),
            ExprKind::Bool(_) => Some(Ty::Bool),
            ExprKind::Str(_) => Some(Ty::Str),
            ExprKind::Unit => Some(Ty::Unit),
            ExprKind::Local(x) => Some(self.local(*x).clone()),
            ExprKind::Unary(_, x) | ExprKind::Try(x) => {
                self.expr(x);
                None
            }
            ExprKind::Binary(op, a, b) => {
                self.expr(a);
                self.expr(b);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => Some(Ty::Int),
                    BinOp::Concat => Some(Ty::Str),
                    _ => Some(Ty::Bool),
                }
            }
            ExprKind::If(c, then, els) => {
                self.expr(c);
                self.block(then);
                els.iter().for_each(|x| self.expr(x));
                None
            }
            ExprKind::Block(b) => {
                self.block(b);
                None
            }
            ExprKind::Call(f, args) => {
                let g = &self.m.funcs[f.0 as usize];
                assert_eq!(args.len(), g.params as usize);
                for (a, l) in args.iter().zip(&g.locals) {
                    self.expr(a);
                    self.agree(&a.ty, &l.ty, "argument");
                }
                Some(g.ret.clone())
            }
            ExprKind::CapCall(x, i, args) => {
                let Ty::Cap(c) = self.local(*x) else {
                    panic!("not a capability")
                };
                let (_, params, ret) = &self.m.caps[c.0 as usize].methods[*i as usize];
                for (a, t) in args.iter().zip(params) {
                    self.expr(a);
                    self.agree(&a.ty, t, "capability argument");
                }
                Some(ret.clone())
            }
            ExprKind::Builtin(_, recv, args) => {
                self.expr(recv);
                args.iter().for_each(|a| self.expr(a));
                None
            }
            ExprKind::Mutate(p, _, args) => {
                self.place(p);
                args.iter().for_each(|a| self.expr(a));
                Some(Ty::Unit)
            }
            ExprKind::List(items) => {
                let Ty::List(t) = &e.ty else {
                    panic!("list of {:?}", e.ty)
                };
                for x in items {
                    self.expr(x);
                    self.agree(&x.ty, t, "element");
                }
                None
            }
            ExprKind::MapNew => None,
            ExprKind::StructLit(s, inits) => {
                let def = &self.m.structs[s.0 as usize];
                assert_eq!(inits.len(), def.fields.len());
                for (i, x) in inits {
                    self.expr(x);
                    self.agree(&x.ty, &def.fields[*i as usize].1, "field");
                }
                Some(Ty::Struct(*s))
            }
            ExprKind::Field(base, i) => {
                self.expr(base);
                let Ty::Struct(s) = &base.ty else {
                    panic!("field of {:?}", base.ty)
                };
                Some(self.m.structs[s.0 as usize].fields[*i as usize].1.clone())
            }
            ExprKind::Index(base, i) => {
                self.expr(base);
                self.expr(i);
                match &base.ty {
                    Ty::List(t) => Some((**t).clone()),
                    _ => Some(Ty::Int),
                }
            }
            ExprKind::Variant(en, i, args) => {
                args.iter().for_each(|a| self.expr(a));
                if let EnumRef::User(id) = en {
                    let payload = &self.m.enums[id.0 as usize].variants[*i as usize].1;
                    for (a, t) in args.iter().zip(payload) {
                        self.agree(&a.ty, t, "payload");
                    }
                    Some(Ty::Enum(*id))
                } else {
                    None
                }
            }
            ExprKind::Match(scrut, arms) => {
                self.expr(scrut);
                for arm in arms {
                    self.expr(&arm.body);
                }
                None
            }
        };
        if let Some(want) = want {
            self.agree(&e.ty, &want, "expression");
        }
    }
}

/// Every program in lang/tests and examples/ that checks also lowers, with consistent types.
#[test]
fn every_test_program_and_example_lowers() {
    let n = common::big_stack(|| {
        let sources = corpus::sources(std::path::Path::new(".."));
        let caps = CapTable::v0();
        let mut n = 0;
        for files in &sources {
            let pairs: Vec<_> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            let Ok(p) = check_files(&pairs, &caps) else { continue };
            let m = lower(&p, &caps);
            for f in &m.funcs {
                Types { m: &m, f }.block(&f.body);
            }
            n += 1;
        }
        n
    });
    assert!(n > 350, "only {n} programs lowered");
}
