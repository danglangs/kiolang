//! The kio programs the tests and examples hold, shared by the lowering and differential tests.
#![allow(dead_code)]
use std::path::Path;

/// Every program under `root` (the repository): string literals in `lang/tests/*.rs`, the
/// `.kio` examples, and the two-file `web` and `serve` apps, each as `(module, source)` files.
pub fn sources(root: &Path) -> Vec<Vec<(String, String)>> {
    let mut sources: Vec<Vec<(String, String)>> = Vec::new();
    for e in std::fs::read_dir(root.join("lang/tests")).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x == "rs") {
            let src = std::fs::read_to_string(&path).unwrap();
            sources.extend(programs(&src).into_iter().map(|p| vec![("main".into(), p)]));
        }
    }
    for dir in ["examples", "examples/tour"] {
        for e in std::fs::read_dir(root.join(dir)).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "kio") {
                sources.push(vec![("main".into(), std::fs::read_to_string(path).unwrap())]);
            }
        }
    }
    for dir in ["examples/web", "examples/serve"] {
        let read = |f: &str| std::fs::read_to_string(root.join(dir).join(f)).unwrap();
        sources.push(vec![("app".into(), read("app.kio")), ("web".into(), read("web.kio"))]);
    }
    sources
}

/// Every Rust string literal in `src`, unescaped, with the name of the `const` it initializes.
fn literals(src: &str) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let raw = c == 'r' && chars.peek().is_some_and(|&(_, c)| c == '#');
        if c == '/' && chars.peek().is_some_and(|&(_, c)| c == '/') {
            chars.by_ref().find(|&(_, c)| c == '\n');
            continue;
        }
        if !raw && c != '"' {
            continue;
        }
        if raw && (chars.next(), chars.next()).1.map(|(_, c)| c) != Some('"') {
            continue;
        }
        let mut s = String::new();
        while let Some((_, c)) = chars.next() {
            match c {
                '"' if !raw => break,
                '"' if chars.peek().is_some_and(|&(_, c)| c == '#') => {
                    chars.next();
                    break;
                }
                '\\' if !raw => match chars.next().map(|(_, c)| c) {
                    Some('n') => s.push('\n'),
                    Some('\n') => {
                        while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    Some(c) => s.push(c),
                    None => {}
                },
                c => s.push(c),
            }
        }
        let before = src[..at].trim_end().strip_suffix(": &str =");
        let name = before.and_then(|b| b.rsplit_once("const ")).map(|(_, n)| n.to_string());
        out.push((name, s));
    }
    out
}

/// The kio programs in a test file: its literals, and `format!` strings with `{NAME}` filled
/// from the file's `const NAME: &str`.
pub fn programs(src: &str) -> Vec<String> {
    let lits = literals(src);
    let mut out = Vec::new();
    for (_, l) in &lits {
        let mut s = l.replace("{{", "{").replace("}}", "}");
        for (name, value) in &lits {
            if let Some(name) = name {
                s = s.replace(&format!("{{{name}}}"), value);
            }
        }
        out.extend([l.clone(), s]);
    }
    out
}
