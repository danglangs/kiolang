use super::*;

fn module(src: &str) -> Module {
    let caps = lang::CapTable::v0();
    let p = lang::check(src, &caps).unwrap();
    lang::lower::lower(&p, &caps)
}

#[test]
fn the_component_validates() {
    let wasm = compile(&module("fn main() -> Int { 40 + 2 }")).unwrap();
    wasmparser::Validator::new().validate_all(&wasm.component).unwrap();
    assert_eq!(
        wasm.sites,
        [Site {
            span: Span {
                start: 19,
                end: 25,
                file: 0
            },
            template: "Int overflow"
        }]
    );
}

#[test]
#[should_panic(expected = "codegen: expression Builtin")]
fn unsupported_ir_names_the_node() {
    let _ = compile(&module("fn main() -> Int { \"ab\".len() }"));
}

#[test]
fn a_site_formats_its_template() {
    let span = Span {
        start: 0,
        end: 1,
        file: 0,
    };
    let s = Site {
        span,
        template: "index {a} out of bounds for length {b}",
    };
    assert_eq!(s.message(-1, 3), "index -1 out of bounds for length 3");
}
