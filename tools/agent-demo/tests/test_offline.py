"""The tool, the checkers and the stand-in API without a model.

    KIO=target/release/kio tools/agent-demo/.venv/bin/python -m unittest discover tools/agent-demo/tests
"""

import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "kio-py" / "src"))

from kio_run import Kio  # noqa: E402

from agent_demo import standin  # noqa: E402
from agent_demo.agent import Session, authority_report, build_agent, run_kio_impl, run_request  # noqa: E402
from agent_demo.jobs import JOBS  # noqa: E402

DATA = HERE / "data"
JOB = {j.id: j for j in JOBS}

# A hand-written answer to 01-totals (and, with the rate, 11-http-eur).
TOTALS = """
fn digits(s: Str) -> Bool {
  if s == "" { return false; }
  for c in s.chars() { if c < "0" || c > "9" { return false; } }
  true
}

fn cents(a: Str) -> Option<Int> {
  let p = a.split(".");
  if p.len() > 2 || !digits(p[0]) { return None; }
  let whole = p[0].parse_int()?;
  if p.len() == 1 { return Some(whole * 100); }
  if !digits(p[1]) || p[1].len() > 2 { return None; }
  let f = p[1].parse_int()?;
  Some(whole * 100 + if p[1].len() == 1 { f * 10 } else { f })
}

fn valid_date(d: Str) -> Bool {
  let p = d.split("-");
  if d.len() != 10 || p.len() != 3 || !digits(p[0]) || !digits(p[1]) || !digits(p[2]) { return false; }
  let m = p[1].parse_int().unwrap_or(0);
  let day = p[2].parse_int().unwrap_or(0);
  p[0].len() == 4 && m >= 1 && m <= 12 && day >= 1 && day <= 31
}

fn money(c: Int) -> Str {
  let r = c % 100;
  (c / 100).to_str() + "." + (if r < 10 { "0" } else { "" }) + r.to_str()
}

fn totals(input: Dir) -> Result<Map<Str, Int>, IoErr> {
  let lines = input.read("orders.csv")?.to_str().lines();
  let mut seen: Map<Str, Bool> = Map::new();
  let mut t: Map<Str, Int> = Map::new();
  for line in lines.slice(1, lines.len()) {
    if line.trim() == "" { continue; }
    let f = line.split(",").map(|x| x.trim());
    if f.len() != 5 || f[0] == "" || f[1] == "" || !valid_date(f[2]) { continue; }
    let Some(c) = cents(f[3]) else { continue; };
    if f[4] != "paid" && f[4] != "pending" && f[4] != "refunded" { continue; }
    if seen.get(f[0]).is_some() { continue; }
    seen.insert(f[0], true);
    if f[4] == "paid" { t.insert(f[1], t.get(f[1]).unwrap_or(0) + c); }
  }
  Ok(t)
}
"""

JOB01 = TOTALS + """
fn main(input: Dir, out: Dir) -> Result<Str, IoErr> {
  let t = totals(input)?;
  let mut csv = "customer_id,total\\n";
  for k in t.keys() { csv = csv + k + "," + money(t.get(k).unwrap_or(0)) + "\\n"; }
  out.write("totals.csv", csv.to_bytes())?;
  Ok(t.len().to_str() + " customers")
}
"""

JOB11 = TOTALS + """
enum Failed { Io(IoErr), Http(HttpErr), Bad(Str) }

fn main(input: Dir, out: Dir, api: Http) -> Result<Str, Failed> {
  let t = totals(input).map_err(|e| Failed::Io(e))?;
  let body = api.get("/v1/rates?base=USD").map_err(|e| Failed::Http(e))?.to_str();
  let doc = body.parse_json().map_err(|e| Failed::Bad(e))?;
  let Some(rate) = doc.get("rates").and_then(|r| r.get("EUR")).and_then(|v| v.as_float()) else {
    return Err(Failed::Bad("no EUR rate"));
  };
  let mut csv = "customer_id,total_eur\\n";
  for k in t.keys() {
    let eur = (t.get(k).unwrap_or(0).to_float() * rate).round().to_int().unwrap_or(0);
    csv = csv + k + "," + money(eur) + "\\n";
  }
  out.write("totals_eur.csv", csv.to_bytes()).map_err(|e| Failed::Io(e))?;
  Ok("ok")
}
"""


class Offline(unittest.TestCase):
    def setUp(self):
        self.kio = Kio(os.environ.get("KIO", "kio"))
        self.tmp = tempfile.TemporaryDirectory()
        self.out = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_job01_passes_its_checker(self):
        s = Session(self.kio, DATA, self.out)
        text = run_kio_impl(s, JOB01)
        self.assertIn("RUN OK", text)
        self.assertIn("totals.csv", text)
        self.assertIn("input.read x1 ok", text)
        self.assertEqual(JOB["01-totals"].check(DATA, self.out, s.last_run()["value"]), "")

    def test_check_errors_go_back_and_nothing_runs(self):
        s = Session(self.kio, DATA, self.out)
        text = run_kio_impl(s, 'fn main(input: Dir, out: Dir) -> Result<Str, IoErr> { Ok(1) }')
        self.assertTrue(text.startswith("CHECK FAILED"), text)
        self.assertIsNone(s.last_run())

    def test_wrong_signature_names_the_fixed_grants(self):
        s = Session(self.kio, DATA, self.out)
        text = run_kio_impl(s, 'fn main(data: Dir) -> Result<Str, IoErr> { Ok("x") }')
        self.assertIn("declare exactly `fn main(input: Dir, out: Dir)`", text)

    def test_input_is_read_only(self):
        s = Session(self.kio, DATA, self.out)
        src = 'fn main(input: Dir, out: Dir) -> Result<Str, IoErr> { input.write("x", "1".to_bytes())?; Ok("") }'
        text = run_kio_impl(s, src)
        self.assertIn("FAILED", text)
        self.assertFalse((DATA / "x").exists())

    def test_wrong_output_fails_the_checker(self):
        (self.out / "totals.csv").write_text("customer_id,total\nC001,1.00\n")
        self.assertNotEqual(JOB["01-totals"].check(DATA, self.out, None), "")

    def test_agent_loop_with_a_scripted_model(self):
        from pydantic_ai.messages import ModelResponse, TextPart, ToolCallPart, ToolReturnPart
        from pydantic_ai.models.function import FunctionModel

        seen = []

        def script(messages, info):
            returns = [p for m in messages for p in getattr(m, "parts", []) if isinstance(p, ToolReturnPart)]
            if not returns:
                return ModelResponse(parts=[ToolCallPart("run_kio", {"source": "fn main(input: Dir, out: Dir) -> Int { 1 + \"a\" }"})])
            seen.append(returns[-1].content)
            if len(returns) == 1:
                return ModelResponse(parts=[ToolCallPart("run_kio", {"source": JOB01})])
            return ModelResponse(parts=[TextPart("wrote totals.csv")])

        agent = build_agent("not-a-key")
        s = Session(self.kio, DATA, self.out)
        with agent.override(model=FunctionModel(script)):
            answer, usage, failure = run_request(agent, s, JOB["01-totals"].request)
        self.assertIsNone(failure)
        self.assertEqual(answer, "wrote totals.csv")
        self.assertEqual(usage.tool_calls, 2)
        self.assertTrue(seen[0].startswith("CHECK FAILED"), seen[0])
        self.assertIn("RUN OK", seen[1])
        self.assertEqual(JOB["01-totals"].check(DATA, self.out, s.last_run()["value"]), "")
        self.assertIn("input: Dir  (read-only)", authority_report(s))

    def test_http_job_key_injected_by_host(self):
        log = self.out / "api.log"
        out = self.out / "o"
        out.mkdir()
        with standin.running(log) as base:
            s = Session(self.kio, DATA, out, http_base=base)
            text = run_kio_impl(s, JOB11)
            key = os.environ[standin.VAR]
        self.assertIn("RUN OK", text, text)
        self.assertEqual(JOB["11-http-eur"].check(DATA, out, None), "")
        self.assertIn('"auth_ok": true', log.read_text())
        self.assertNotIn(key, text + (out / "totals_eur.csv").read_text())


if __name__ == "__main__":
    unittest.main()
