//! C8c8: a `for` variable, or a `match` binding of a scrutinee local, that the body reads only
//! in place is the slot's own pointer, with no reference of its own; one that is stored,
//! returned, captured or bound by `let` still copies. Every program runs compiled and
//! interpreted and must agree (and leave no object, which the test-heap runtime checks).

use kio_host::{Interp, Kio, Limits};

type Run = Result<String, (String, usize, usize)>;

fn limits() -> Limits {
    Limits {
        fuel: 20_000_000_000,
        ..Limits::default()
    }
}

fn compiled(src: &str) -> Run {
    match Kio::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn interpreted(src: &str) -> Run {
    match Interp::new(limits()).unwrap().run(src, vec![]).unwrap() {
        Ok(out) => Ok(out.value),
        Err(d) => Err((d[0].message.clone(), d[0].start as usize, d[0].end as usize)),
    }
}

fn both(src: &str) -> Run {
    let c = compiled(src);
    assert_eq!(c, interpreted(src), "compiled vs interpreted: {src}");
    c
}

fn value(src: &str) -> String {
    both(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

fn error(src: &str) -> (String, usize, usize) {
    both(src).unwrap_err()
}

/// Every use of a `for` variable: read in place (`+=`, `==`, `len`, a borrowed argument, a
/// `match` scrutinee, `?`, an inner loop's iterable) while the list is pushed to, popped,
/// written by index and reassigned in the body; moved out by `let`; stored in a list and a
/// map; captured; returned; `break`, `continue`, `?` and `return` mid-loop; nested loops over
/// the same list and over an element's list; elements that are `Str`, `Option<Str>`, a mixed
/// one-slot enum and an unboxed struct with a pointer field; a loop over a borrowed parameter,
/// a moved local and a field.
#[test]
fn borrowed_loop_variables_in_every_position() {
    let src = r#"enum J { Null, N(Int), S(Str) }
        struct P { name: Str, n: Int }
        struct Bag { items: List<Str>, tag: Str }
        fn lent_len(s: Str) -> Int { s.len() }
        fn own_it(s: Str) -> Str { s }
        fn mutated(xs: List<Str>) -> Str {
            let mut ys = xs;
            let mut out = "";
            for x in ys {
                out += x;
                ys.push(x + "!");
                if ys.len() > 5 { let z = ys.pop(); }
                ys[0] = "z" + "";
            }
            let mut zs = ys;
            for x in zs {
                zs = [];
                out += x;
                zs.push(x);
            }
            out + ":" + ys.len().to_str() + ":" + zs.len().to_str()
        }
        fn moved_out(xs: List<Str>) -> List<Str> {
            let mut out: List<Str> = [];
            for x in xs {
                let y = x;
                out.push(y);
            }
            out
        }
        fn stored(xs: List<Str>) -> Int {
            let mut m: Map<Str, Str> = Map::new();
            let mut ys: List<Str> = [];
            for x in xs { m.insert(x, x); ys.push(x); }
            m.len() * 100 + ys.len()
        }
        fn captured(xs: List<Str>) -> Int {
            let mut total = 0;
            for x in xs { let f = |d: Int| x.len() + d; total += f(1); }
            total
        }
        fn returned(xs: List<Str>, want: Str) -> Str {
            for x in xs { if x == want { return x; } }
            "none" + ""
        }
        fn exits(xs: List<Option<Str>>) -> Option<Int> {
            let mut n = 0;
            for x in xs {
                let s = x?;
                if s == "skip" { continue; }
                if s == "stop" { break; }
                n += s.len() + lent_len(s) + own_it(s).len();
            }
            Some(n)
        }
        fn nested(xs: List<Str>) -> Int {
            let mut n = 0;
            for a in xs { for b in xs { if a == b { n += 1; } else { n += a.len() * b.len(); } } }
            n
        }
        fn rows(rs: List<List<Str>>) -> Int {
            let mut n = 0;
            let mut rs2 = rs;
            for r in rs2 {
                rs2.push([]);
                for c in r { n += c.len(); }
                for c in r { n += lent_len(c); }
            }
            n * 10 + rs2.len()
        }
        fn mixed(js: List<J>) -> Int {
            let mut n = 0;
            for j in js { n += match j { J::Null => 1, J::N(v) => v, J::S(s) => s.len() }; }
            n
        }
        fn structs(ps: List<P>) -> Str {
            let mut out = "";
            for p in ps { out += p.name; out += p.n.to_str(); }
            out
        }
        fn field(b: Bag) -> Int {
            let mut n = 0;
            for it in b.items { n += it.len() + b.tag.len(); }
            n
        }
        fn main() -> Str {
            let xs = ["a" + "", "bb" + "", "ccc" + ""];
            let os = [Some("ab" + ""), Some("skip" + ""), Some("c" + ""), Some("stop" + ""), Some("zzz" + "")];
            let os2 = [Some("ab" + ""), None, Some("c" + "")];
            let js = [J::Null, J::N(5), J::S("four" + "")];
            let ps = [P { name: "p" + "", n: 1 }, P { name: "q" + "", n: 2 }];
            let bag = Bag { items: ["x" + "", "yy" + ""], tag: "t" + "" };
            let moved = ["m" + "", "nn" + ""];
            let mut n = 0;
            for x in moved { n += x.len(); }
            mutated(xs) + "|" + moved_out(xs).join(",") + "|" + stored(xs).to_str()
                + "|" + captured(xs).to_str() + "|" + returned(xs, "bb" + "") + returned(xs, "q" + "")
                + "|" + exits(os).unwrap_or(-1).to_str() + "," + exits(os2).unwrap_or(-1).to_str()
                + "|" + nested(xs).to_str() + "|" + rows([xs, moved_out(xs)]).to_str()
                + "|" + mixed(js).to_str() + "|" + structs(ps) + "|" + field(bag).to_str() + "|" + n.to_str()
        }"#;
    assert_eq!(
        value(src),
        "abbccczbbccca!bb!:5:1|a,bb,ccc|303|9|bbnone|9,-1|25|244|10|p1q2|5|3"
    );
}

/// `match` bindings: of a borrowed parameter (`fold`), of a borrowed `for` variable, of a
/// local no arm mentions (borrowed), of a local an arm reassigns (copied), of a local the
/// match's own assignment overwrites, of a `|` pattern, nested, and bound whole.
#[test]
fn borrowed_match_bindings() {
    let src = r#"enum Tree { Leaf(Int), Node(List<Tree>) }
        enum K { A(Str), B(Str), C(Int) }
        fn build(d: Int, v: Int) -> Tree {
            if d == 0 { Tree::Leaf(v) } else { Tree::Node([build(d - 1, v * 2), build(d - 1, v * 2 + 1)]) }
        }
        fn fold(t: Tree) -> Int {
            match t {
                Tree::Leaf(v) => v,
                Tree::Node(kids) => { let mut s = 0; for k in kids { s += fold(k); } s }
            }
        }
        fn count(t: Tree) -> Int {
            match t {
                Tree::Leaf(v) => 1,
                Tree::Node(kids) => { let kept = kids; let mut n = 0; for k in kept { n += count(k); } n + kids.len() }
            }
        }
        fn local_bindings(o: Option<Str>) -> Int {
            let mut o2 = o;
            let a = match o2 { Some(s) => s.len(), None => 0 };
            let b = match o2 { Some(s) => { o2 = Some("longer" + ""); s.len() }, None => 0 };
            let c = match o2 { Some(s) => s.len(), None => 7 };
            o2 = match o2 { Some(s) => Some(s + "!"), None => None };
            let d = match o2 { Some(s) => s.len(), None => 0 };
            a + b * 10 + c * 100 + d * 1000
        }
        fn alts(ks: List<K>) -> Int {
            let mut n = 0;
            for k in ks { n += match k { K::A(s) | K::B(s) => s.len(), K::C(i) => i }; }
            n
        }
        fn nested(o: Option<Option<Str>>) -> Int {
            match o { Some(Some(s)) => s.len(), Some(None) => 1, None => 0 }
        }
        fn whole(o: Option<Str>) -> Int {
            match o { w => match w { Some(s) => s.len(), None => 0 } }
        }
        fn main() -> Int {
            let t = build(6, 1);
            let ks = [K::A("aa" + ""), K::B("bbb" + ""), K::C(4)];
            fold(t) * 1000000 + count(t) * 10000 + local_bindings(Some("abc" + "")) + alts(ks)
                + nested(Some(Some("xy" + ""))) + nested(Some(None)) + whole(Some("q" + ""))
        }"#;
    // fold 6112 (leaves 64..127), count 64 leaves + 63 nodes * 2 kids = 190, local_bindings
    // 3 + 30 + 600 + 7000, alts 2 + 3 + 4, nested 2 and 1, whole 1.
    assert_eq!(value(src), "6113907646");
}

/// An error in the body mid-loop (an index out of bounds on a borrowed element, an overflow)
/// is reported at its span, both ways, with the loop's and the arm's references released.
#[test]
fn borrowed_loop_errors() {
    let src = r#"fn main() -> Int {
            let rows = [[1, 2], [3]];
            let mut n = 0;
            for r in rows { n += r[0] + r[1]; }
            n
        }"#;
    let (msg, start, end) = error(src);
    assert!(msg.contains("out of bounds") || msg.contains("index"), "{msg}");
    assert_eq!(&src[start..end], "r[1]");
    let src = r#"enum J { N(Int), S(Str) }
        fn main() -> Int {
            let js = [J::S("a" + ""), J::N(9223372036854775807)];
            let mut n = 0;
            for j in js { n += match j { J::N(v) => v + 1, J::S(s) => s.len() }; }
            n
        }"#;
    let (msg, start, end) = error(src);
    assert!(msg.contains("overflow"), "{msg}");
    assert_eq!(&src[start..end], "v + 1");
}
