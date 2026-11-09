enum Tree {
    Leaf(i64),
    Node(Vec<Tree>),
}

struct Stats {
    sum: i64,
    leaves: i64,
    depth: i64,
}

fn build(d: i64, v: i64) -> Tree {
    if d == 0 {
        Tree::Leaf(v % 1000)
    } else {
        Tree::Node(vec![build(d - 1, v * 2), build(d - 1, v * 2 + 1)])
    }
}

fn fold(t: &Tree) -> Stats {
    match t {
        Tree::Leaf(v) => Stats {
            sum: *v,
            leaves: 1,
            depth: 0,
        },
        Tree::Node(kids) => {
            let mut s = Stats {
                sum: 0,
                leaves: 0,
                depth: 0,
            };
            for k in kids {
                let c = fold(k);
                s.sum += c.sum;
                s.leaves += c.leaves;
                s.depth = s.depth.max(c.depth + 1);
            }
            s
        }
    }
}

fn main() {
    let s = fold(&build(std::hint::black_box(20), 1));
    println!("{}", s.sum * 1000 + s.leaves + s.depth);
}
