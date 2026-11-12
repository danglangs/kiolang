# kio by example

kio is Rust syntax without references, traits, tuples or macros: values are copied, and everything a program touches (files, hosts, stores) is a parameter of `main`. Each snippet below is a whole program that passes `kio check`; copy the shape. The reference is [language.md](language.md); E4's `cookbook` prompt is this file followed by it.

## Rust habits that kio rejects

| Rust | kio |
|---|---|
| `&str`, `String`, `Vec<T>`, `HashMap` from std | `Str`, `List<T>`, `Map<K, V>` (key order) or `HashMap<K, V>` (insertion order) |
| `format!("{} {}", a, n)` | `a + " " + n.to_str()` |
| `s.chars().nth(i)`, `s[i..j]` | `s.chars()[i]` (a one-character `Str`), `s.slice(i, j)` (byte offsets) |
| `x as f64`, `x as i64` | `x.to_float()`, `f.to_int().unwrap_or(0)` |
| `.iter().map(..).collect()` | `xs.map(..)`, `xs.filter(..)` directly on the `List` |
| `for (k, v) in &m` | `for k in m.keys() { let v = m.get(k).unwrap_or(0); .. }` |
| tuples, `(a, b)` | a `struct` |
| `impl`, traits, `println!`, `use` | free functions; `main` returns the output |
| `let v = vec![]` | `let mut v: List<Int> = [];` (empty values need a type) |
| `.unwrap()`, `.expect(..)` | `?`, `unwrap_or(..)`, `let Some(x) = .. else { .. };` or `match` |
| `x.len() as i64`, `usize` | everything is `Int` |
| `break value` | a `let mut` set before `break` |

## Lines and counts

```
struct Count { word: Str, n: Int }

fn main(input: Dir) -> Result<Str, IoErr> {
  let text = input.read("words.txt")?.to_str();
  let mut counts: Map<Str, Int> = Map::new();
  for line in text.lines() {
    let w = line.trim().to_lowercase();
    if w == "" { continue; }
    counts.insert(w, counts.get(w).unwrap_or(0) + 1);
  }
  let mut rows: List<Count> = [];
  for w in counts.keys() { rows.push(Count { word: w, n: counts.get(w).unwrap_or(0) }); }
  rows.sort_by(|a, b| a.n > b.n || (a.n == b.n && a.word < b.word));
  Ok(rows.map(|r| r.word + " " + r.n.to_str()).join("\n"))
}
```

## Parse rows, skip bad ones

```
struct Row { name: Str, cents: Int }

fn parse(line: Str) -> Option<Row> {
  let f = line.split(",");
  if f.len() != 2 { return None; }
  Some(Row { name: f[0].trim(), cents: f[1].trim().parse_int()? })
}

fn money(cents: Int) -> Str {
  let c = cents % 100;
  (cents / 100).to_str() + "." + (if c < 10 { "0" } else { "" }) + c.to_str()
}

fn main(input: Dir) -> Result<Str, IoErr> {
  let lines = input.read("rows.csv")?.to_str().lines();
  let mut total = 0;
  let mut bad = 0;
  for line in lines.slice(1, lines.len()) {          // skip the header
    let Some(r) = parse(line) else { bad += 1; continue; };
    total += r.cents;
  }
  Ok("total " + money(total) + "\nbad " + bad.to_str())
}
```

## Several error types: one enum and `map_err`

`?` needs the function's error type exactly; there is no `From`.

```
enum Failed { Io(IoErr), Json(Str), Kv(KvErr) }

fn main(input: Dir, state: Kv) -> Result<Str, Failed> {
  let text = input.read("orders.json").map_err(|e| Failed::Io(e))?.to_str();
  let doc = text.parse_json().map_err(|e| Failed::Json(e))?;
  let n = doc.as_list().unwrap_or([]).len();
  let seen = state.add("orders", n).map_err(|e| Failed::Kv(e))?;
  Ok(n.to_str() + " new, " + seen.to_str() + " total")
}
```

## JSON in and out

```
fn total(order: Json) -> Int {
  let mut t = 0;
  for item in order.get("items").and_then(|v| v.as_list()).unwrap_or([]) {
    let qty = item.get("qty").and_then(|v| v.as_int()).unwrap_or(0);
    let price = item.get("price").and_then(|v| v.as_int()).unwrap_or(0);
    t += qty * price;
  }
  t
}

fn main(input: Dir) -> Result<Str, IoErr> {
  let text = input.read("orders.json")?.to_str();
  let Ok(doc) = text.parse_json() else { return Ok("bad json"); };
  let mut out: List<Json> = [];
  for order in doc.as_list().unwrap_or([]) {
    let mut m: Map<Str, Json> = Map::new();
    m.insert("id", order.get("id").unwrap_or(Json::Null));
    m.insert("total", Json::Num(total(order)));
    out.push(Json::Obj(m));
  }
  Ok(Json::Arr(out).to_json())
}
```

## Files out

```
fn main(input: Dir, out: Dir) -> Result<Str, IoErr> {
  let mut n = 0;
  for name in input.list("")? {
    if !name.ends_with(".txt") { continue; }
    let body = input.read(name)?.to_str().to_uppercase();
    out.write(name, body.to_bytes())?;              // create or replace
    n += 1;
  }
  Ok(n.to_str() + " files")
}
```

## HTTP client: `get`, then `send` with a status check

```
enum Failed { Http(HttpErr), Json(Str), Status(Int) }

fn main(api: Http) -> Result<Str, Failed> {
  let body = api.get("/items?page=1").map_err(|e| Failed::Http(e))?.to_str();
  let items = body.parse_json().map_err(|e| Failed::Json(e))?.as_list().unwrap_or([]);
  let mut m: Map<Str, Json> = Map::new();
  m.insert("count", Json::Num(items.len()));
  let r = api.send("POST", "/summary", ["content-type: application/json"],
                   Json::Obj(m).to_json().to_bytes(), 0).map_err(|e| Failed::Http(e))?;
  if r.status != 200 && r.status != 201 { return Err(Failed::Status(r.status)); }
  Ok("posted " + items.len().to_str())
}
```

## Kv: state between runs

```
fn main(input: Dir, store: Kv) -> Result<Str, KvErr> {
  let Ok(text) = input.read("event.txt") else { return Ok("no event"); };
  let key = "user:" + text.to_str().trim();
  let seen = match store.get(key)? {
    Some(b) => b.to_str().parse_int().unwrap_or(0),
    None => 0,
  };
  store.set(key, (seen + 1).to_str().to_bytes())?;
  let mut users: List<Str> = [];
  for k in store.keys("user:")? { users.push(k.strip_prefix("user:").unwrap_or(k)); }
  Ok(users.join(","))
}
```

## Db: schema, parameters, reading columns

```
fn text(row: List<SqlValue>, i: Int) -> Str { match row[i] { SqlValue::Text(s) => s, _ => "" } }
fn int(row: List<SqlValue>, i: Int) -> Int { match row[i] { SqlValue::Int(n) => n, _ => 0 } }

fn main(db: Db) -> Result<Str, DbErr> {
  db.exec("CREATE TABLE IF NOT EXISTS items (name TEXT PRIMARY KEY, qty INTEGER NOT NULL)", [])?;
  db.exec("INSERT INTO items (name, qty) VALUES (?, ?) ON CONFLICT(name) DO UPDATE SET qty = qty + excluded.qty",
          [SqlValue::Text("apple"), SqlValue::Int(3)])?;
  let rows = db.query("SELECT name, qty FROM items WHERE qty > ? ORDER BY name", [SqlValue::Int(0)])?;
  Ok(rows.map(|r| text(r, 0) + " " + int(r, 1).to_str()).join("\n"))
}
```

## HTTP handler (`--listen`)

`main` runs once per request. Route on method and path; send one response.

```
struct Reply { status: Int, body: Json }

fn header(req: Incoming, name: Str) -> Option<Str> {
  let names = req.header_names();
  let values = req.header_values();
  for i in 0..names.len() { if names[i] == name { return Some(values[i]); } }
  None
}

fn query(target: Str, key: Str) -> Option<Str> {
  let q = target.split("?");
  if q.len() < 2 { return None; }
  for pair in q[1].split("&") {
    if let Some(v) = pair.strip_prefix(key + "=") { return Some(v); }
  }
  None
}

fn error(status: Int, msg: Str) -> Reply {
  let mut m: Map<Str, Json> = Map::new();
  m.insert("error", Json::Str(msg));
  Reply { status: status, body: Json::Obj(m) }
}

fn route(req: Incoming, store: Kv) -> Result<Reply, KvErr> {
  let path = req.target().split("?")[0];
  let parts = path.split("/");                       // "/notes/7" is ["", "notes", "7"]
  match req.method() {
    "GET" => if path == "/count" {
      let n = store.add("hits", 1)?;
      let who = query(req.target(), "who").unwrap_or("anon");
      let mut m: Map<Str, Json> = Map::new();
      m.insert("hits", Json::Num(n));
      m.insert("who", Json::Str(who));
      Ok(Reply { status: 200, body: Json::Obj(m) })
    } else { Ok(error(404, "not found")) },
    "POST" => if parts.len() == 3 && parts[1] == "notes" {
      let Some(id) = parts[2].parse_int() else { return Ok(error(404, "not found")); };
      if header(req, "x-token").is_none() { return Ok(error(401, "no token")); }
      let Ok(body) = req.body().to_str().parse_json() else { return Ok(error(400, "bad json")); };
      store.set("note:" + id.to_str(), body.to_json().to_bytes())?;
      Ok(Reply { status: 201, body: body })
    } else { Ok(error(404, "not found")) },
    _ => Ok(error(405, "method not allowed")),
  }
}

fn main(req: Incoming, res: Outgoing, store: Kv) -> Result<(), WebErr> {
  let r = route(req, store).unwrap_or(error(500, "store failed"));
  res.send(r.status, ["content-type"], ["application/json"], r.body.to_json().to_bytes())
}
```

## Tests: `#[test]`, `assert`, `assert_eq`

A test is a top-level function marked `#[test]`; `kio test` runs each one. `assert` takes a message; `assert_eq` prints both values when they differ. A test's parameters are capabilities only, and it returns `()` or `Result<(), E>`.

```
struct Row { name: Str, qty: Int }

fn parse(line: Str) -> Option<Row> {
  let f = line.split(",");
  if f.len() != 2 { return None; }
  Some(Row { name: f[0].trim(), qty: f[1].trim().parse_int()? })
}

#[test]
fn parses_a_row() {
  assert_eq(parse("apple, 3"), Some(Row { name: "apple", qty: 3 }));
  assert(parse("apple").is_none(), "one field is not a row");
}

#[test]
fn reads_the_file(input: Dir) -> Result<(), IoErr> {
  input.write("rows.csv", "a,1".to_bytes())?;
  assert_eq(input.read("rows.csv")?.to_str(), "a,1");
  Ok(())
}

fn main(input: Dir) -> Result<Int, IoErr> {
  let rows = input.read("rows.csv")?.to_str().lines().filter(|l| parse(l).is_some());
  Ok(rows.len())
}
```

## Characters and bytes

```
fn is_digit(c: Int) -> Bool { c >= 48 && c <= 57 }

fn main(input: Dir) -> Result<Str, IoErr> {
  let s = input.read("code.txt")?.to_str();
  let b = s.to_bytes();
  let mut digits = 0;
  for i in 0..b.len() { if is_digit(b[i]) { digits += 1; } }
  let mut depth = 0;
  for c in s.chars() {
    match c { "(" => depth += 1, ")" => depth -= 1, _ => {} }
  }
  Ok(digits.to_str() + " " + depth.to_str())
}
```
