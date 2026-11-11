# kio-run (provisional name, unpublished)

Python wrapper over `kio ... --json`. Standard library only, Python 3.10+, needs a `kio` binary.

    pip install "git+https://github.com/danglangs/kiolang#subdirectory=tools/kio-py"

```python
from kio_run import Kio, Grants, Limits

kio = Kio("kio")
src = model_output  # kio source as a str (or a path); modules={"web": text} for `mod web;`
checked = kio.check(src)
if not checked.ok:
    feedback = checked.error_text()  # place, message, hint: send back to the model
else:
    grants = Grants().dir("input", "/data/orders", ro=True).dir("out", "/data/out")
    r = kio.run(src, grants, limits=Limits(fuel=50_000_000))  # fresh temp --audit per run
    print(r.ok, r.value_json(), r.used, r.audit_records, r.errors)
```

`Result` mirrors D2's object (`ok value errors report diff audit used`), plus `exit_code`
and `audit_records`. `check(.., base=old)` diffs authority (`exit_code` 3, `diff.added`).

**The application builds `Grants`, never the model.** A model may propose names; paths,
hosts, methods and header secrets (`http(.., headers={"Authorization": "API_ENV_VAR"})`, read
from the environment by kio) are fixed in your code or an operator's grants file
(`Grants.from_file`).

Tests: `KIO=target/release/kio python3 -m unittest discover tools/kio-py/tests`.
