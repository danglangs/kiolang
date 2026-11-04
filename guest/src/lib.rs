wit_bindgen::generate!({ world: "guest", path: "../caps.wit" });

use lang::CapTable;

struct Component;

impl Guest for Component {
    fn check(source: String) -> Result<Report, Vec<Diag>> {
        match lang::check(&source, &CapTable::v0()) {
            Ok(program) => Ok(Report {
                lines: lang::report(&program).lines().map(String::from).collect(),
            }),
            Err(diags) => Err(diags.into_iter().map(diag).collect()),
        }
    }

    fn run(_source: String, _dirs: Vec<DirGrant>, _https: Vec<HttpGrant>) -> Result<(Report, String), Vec<Diag>> {
        Err(vec![Diag {
            start: 0,
            end: 0,
            message: "`run` is not implemented yet".into(),
            hint: None,
        }])
    }
}

fn diag(d: lang::Diag) -> Diag {
    Diag {
        start: d.span.start as u32,
        end: d.span.end as u32,
        message: d.msg,
        hint: d.hint,
    }
}

export!(Component);
