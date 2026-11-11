"""Runs against a built kio: KIO=target/release/kio python3 -m unittest discover tools/kio-py/tests"""

import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from kio_run import Grants, Kio, Limits  # noqa: E402

KIO = os.environ.get("KIO")

READ = 'fn main(input: Dir) -> Result<Str, Str> {\n  Ok(input.read("a.txt").map_err(|_| "no file")?.to_str())\n}\n'
WRITE = 'fn main(out: Dir) -> Result<Str, Str> {\n  out.write("x", "y".to_bytes()).map_err(|_| "no")?;\n  Ok("done")\n}\n'


@unittest.skipUnless(KIO, "set KIO to a built kio binary")
class Tests(unittest.TestCase):
    def setUp(self):
        self.kio = Kio(KIO)
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        (self.dir / "in").mkdir()
        (self.dir / "in" / "a.txt").write_text("hello")
        (self.dir / "out").mkdir()

    def test_ok_with_audit(self):
        g = Grants().dir("input", self.dir / "in", ro=True)
        r = self.kio.run(READ, g)
        self.assertTrue(r.ok, r.errors)
        self.assertEqual(r.value, "hello")
        self.assertEqual(r.exit_code, 0)
        self.assertEqual(r.report, ["main : input read"])
        self.assertIsNotNone(r.used)
        self.assertTrue(r.audit_records)
        self.assertFalse(os.path.exists(r.audit), "temporary audit log is removed")

    def test_no_audit(self):
        r = self.kio.run(READ, Grants().dir("input", self.dir / "in", ro=True), audit=False)
        self.assertTrue(r.ok)
        self.assertEqual(r.audit_records, [])
        self.assertIsNone(r.audit)

    def test_value_json(self):
        src = 'fn main() -> Result<Str, Str> {\n  Ok("{\\"n\\": 3}")\n}\n'
        r = self.kio.run(src, Grants())
        self.assertEqual(r.value_json(), {"n": 3})

    def test_err(self):
        src = 'fn main() -> Result<Int, Str> {\n  Err("bad row")\n}\n'
        r = self.kio.run(src, Grants())
        self.assertFalse(r.ok)
        self.assertEqual(r.exit_code, 1)
        self.assertEqual((r.errors[0].kind, r.errors[0].message), ("err", "bad row"))
        self.assertIsNone(r.value)

    def test_check_errors(self):
        r = self.kio.check('fn main() -> Int {\n  1 + "x"\n}\n')
        self.assertFalse(r.ok)
        e = r.errors[0]
        self.assertEqual(e.kind, "check")
        self.assertEqual(e.line, 2)
        self.assertIsNotNone(e.col)
        self.assertIn("check", r.error_text())

    def test_check_ok_and_path(self):
        p = self.dir / "p.kio"
        p.write_text(READ)
        r = self.kio.check(p)
        self.assertTrue(r.ok)
        self.assertEqual(r.report, ["main : input read"])
        self.assertTrue(self.kio.check(str(p)).ok)

    def test_modules(self):
        root = "mod web;\nfn main() -> Int {\n  web::double(4)\n}\n"
        r = self.kio.run(root, Grants(), modules={"web": "pub fn double(n: Int) -> Int {\n  n * 2\n}\n"})
        self.assertTrue(r.ok, r.errors)
        self.assertEqual(r.value, "8")

    def test_missing_grant(self):
        r = self.kio.run(READ, Grants())
        self.assertFalse(r.ok)
        e = r.errors[0]
        self.assertEqual(e.kind, "grant")
        self.assertIn("--dir input=", e.hint)

    def test_grant_refusal_ro(self):
        r = self.kio.run(WRITE, Grants().dir("out", self.dir / "out", ro=True))
        self.assertFalse(r.ok)
        self.assertEqual(r.exit_code, 2)
        self.assertEqual(r.errors[0].kind, "grant")
        self.assertFalse((self.dir / "out" / "x").exists())

    def test_write_grant(self):
        r = self.kio.run(WRITE, Grants().dir("out", self.dir / "out"))
        self.assertTrue(r.ok, r.errors)
        self.assertEqual((self.dir / "out" / "x").read_text(), "y")
        self.assertTrue(any("write" in str(rec) for rec in r.audit_records))

    def test_limit(self):
        src = "fn main() -> Int {\n  let mut i = 0;\n  while true { i = i + 1; }\n  i\n}\n"
        r = self.kio.run(src, Grants(), limits=Limits(fuel=100000))
        self.assertFalse(r.ok)
        self.assertEqual(r.errors[0].kind, "fuel")
        self.assertIn("--fuel", r.errors[0].hint)
        self.assertIsNotNone(r.used)

    def test_check_with_grants(self):
        g = Grants().dir("input", self.dir / "in", ro=True)
        self.assertTrue(self.kio.check(READ, grants=g).ok)
        r = self.kio.check(READ, grants=Grants())
        self.assertEqual(r.errors[0].kind, "grant")

    def test_check_base(self):
        old = "fn main() -> Int {\n  1\n}\n"
        new = 'fn main(out: Dir) -> Int {\n  out.write("a", "b".to_bytes());\n  1\n}\n'
        r = self.kio.check(new, base=old)
        self.assertTrue(r.ok)
        self.assertEqual(r.exit_code, 3)
        self.assertTrue(r.diff.added)
        self.assertTrue(any("out" in g for g in r.diff.grants))
        same = self.kio.check(old, base=old)
        self.assertEqual(same.exit_code, 0)
        self.assertFalse(same.diff.added)


class GrantFlags(unittest.TestCase):
    def test_flags(self):
        g = (
            Grants()
            .dir("d", "/x", ro=True)
            .http("api", "example.com", methods=["GET", "POST"], headers={"Authorization": "API_KEY"})
            .kv("k")
            .db("db", "/s.db", ro=True)
            .rng("r", seed=1)
            .clock("c", fixed="2026-11-09T12:00:00Z")
            .log("l", "/l.log")
            .limits(Limits(fuel=5, calls=2))
        )
        self.assertEqual(
            g.args(Limits(fuel=9)),
            ["--dir", "d=/x:ro", "--http", "api=example.com", "--http-methods", "api=GET,POST",
             "--http-header", "api=Authorization:env:API_KEY", "--kv", "k=:memory:", "--db", "db=/s.db:ro",
             "--rng", "r=seed:1", "--clock", "c=fixed:2026-11-09T12:00:00Z", "--log", "l=/l.log",
             "--fuel", "9", "--calls", "2"],
        )


if __name__ == "__main__":
    unittest.main()
