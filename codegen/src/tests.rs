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
            msg: "Int overflow"
        }]
    );
}

#[test]
#[should_panic(expected = "codegen: expression If")]
fn unsupported_ir_names_the_node() {
    let _ = compile(&module("fn main() -> Int { if true { 1 } else { 2 } }"));
}
