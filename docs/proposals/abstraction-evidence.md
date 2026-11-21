# Abstraction: the evidence (L33)

Research only (2026-11-21), no implementation proposal. Sources: kio-web at 63e6a23 (web.kio, api.kio, http.kio, 8 examples, 2,151 lines), [a2-findings.md](a2-findings.md), [a3-findings.md](a3-findings.md), [stdlib-plan.md](stdlib-plan.md) (which already counted A2's and A3's helper lines), F1's friction lists (kio-web examples/flow/README.md), docs/primer.md. The A2 and A3 trees were not reread. Line numbers are kio-web's unless a kio doc is named.

The question for each case: what does a reader lose without the abstraction (lines, duplication, a check), and what is the smallest mechanism that removes it. Three cases are written up; two of them turn out to be one mechanism seen on two surfaces, and the third's answer is "keep what we have".

## Summary

| # | Abstraction | Where it hurts | Independent sources | Mechanism | Verdict |
|---|---|---|---|---|---|
| 1 | A data struct to and from JSON | links, notes x3, payhook, flow; A2, A3 | 4 (kio-web, A2, A3, roadmap) | typed serialization, derive-like, no traits | the one real candidate; design later, after one more held-out app |
| 2 | A SQL row to a struct | flow, payhook, sqlnotes, primer; A2, A3 | 4 | the same mechanism, by position | half solved by L31a's accessors; the rest rides on case 1 |
| 3 | Handlers as values in a route table | 7 of 8 kio-web apps | 1 (the examples copy one README recipe; A2 and A3 did not log it) | `fn` values with capability parameters (abstraction.md option B) | no: the enum costs ~7 lines per app and keeps per-handler authority exact |

Weaker candidates (an idiom, a library function or an existing feature answers them) are in the last table. **Recommendation: no language change yet.**

## Case 1: a data struct to and from JSON

The kio as written. One struct, three hand-written mappings, every field name spelled in each:

```
// links/app.kio:121-135 (encode 7 lines, decode 7), 313-322 (view 10): Link / Record <-> Json
fn encode(l: Link) -> Bytes {
    let mut o: Map<Str, Json> = Map::new();
    o.insert("url", Json::Str(l.url));
    o.insert("created_ms", Json::Num(l.created_ms));
    if let Some(t) = l.ttl_s { o.insert("ttl_s", Json::Num(t)); }
    Json::Obj(o).to_json().to_bytes()
}
fn decode(b: Bytes) -> Option<Record> {
    let j = b.to_str().parse_json().ok()?;
    let url = api::str_field(j, "url")?;
    let created = match j.get("created_ms") { Some(Json::Num(n)) => n, _ => 0 };
    let ttl: Option<Int> = match j.get("ttl_s") { Some(Json::Num(n)) => Some(n), _ => None };
    Some(Record { url: url, created_ms: created, ttl_s: ttl })
}
```

```
// payhook/app.kio:162-174: four fields, 13 lines, the error text written by hand each time
let cents = match api::int_field(o, "amount") {
    Some(c) => c,
    None => return Err("missing integer field \"amount\" in data.object"),
};
```

```
// api.kio:33-43 returns the fields positionally; notes/app.kio:74, 147-148: `store(notes, f[0], f[1])`
let f = match api::str_fields(r, ["title", "body"]) { Ok(f) => f, Err(e) => return e };
```

| Occurrence | Lines | Note |
|---|---|---|
| links `encode`, `decode`, `view`, `create`'s `url`/`ttl_s`/`slug` reads (207-233), `list`'s page object (273-280) | ~50 of 379 | `"created_ms"` and `"ttl_s"` spelled 5 times; a mismatch is a 500 "corrupt record" (145) at run time |
| `note(id, title, body) -> Json`: notes 37-43, kvnotes 35-41, sqlnotes 49-55 | 7 x 3, identical | three apps, one struct never declared |
| payhook `parse_event` + `message` (144-177); `received` (64-68); `deliver`'s `{"text": ..}` (98-101) | ~25 | 5 "missing .. field" texts by hand |
| flow `accepted` (flow.kio:19-23), tests `gen_body` (tests.kio:67-72) | 11 | F1.1 friction 7, "five lines for two fields" |
| `str_fields(r, ["title", "body"])` then `f[0]`, `f[1]` | 15 sites in 3 apps | field order lives in the call and the uses; a swap type-checks |
| A3 F5: ~40 lines of `insert`, about 25% of booking.kio; A2 `field_str`, `field_int`, `object`, `only_fields` (stdlib-plan #2) | 55 `insert`/`Json::Obj` lines across A2 + A3 | held-out apps, independent of kio-web |
| api `Page<T>` (api.kio:68): no app can write `page_json<T>` because `T` has no way to JSON | notes, kvnotes, sqlnotes answer a bare array; links re-encodes `Page` by hand | 3 of 4 paginated apps drop `total`/`offset`/`limit`; whether the missing bound is why is a guess |

What is lost: no check that `encode` and `decode` agree, that a field is read with its declared type, or that `f[1]` is the body. Fixed in kio-web's history: none recorded; stdlib-plan.md lists three bugs in hand-written glue of the same kind (`web::decode`, `web::query`, `json_str`).

What L31a already removed: `Json::obj().with("id", ..)` cuts a builder from 5 lines to 1, `get_str`/`get_int` cut a read to 1. kio-web still uses the 12 `Map::new()` builders (predates L31a). The duplication of names across encode, decode and view, the positional lists and the hand-written error texts remain at one line per field per direction.

| | Encode | Decode |
|---|---|---|
| Python | `json.dumps(dataclasses.asdict(l))` | `Link(**json.loads(s))`; pydantic `Link.model_validate_json(s)` checks types |
| Go | `json.Marshal(l)` with `` `json:"created_ms"` `` tags | `json.Unmarshal(b, &l)` |
| TypeScript | `JSON.stringify(l)` | `JSON.parse(s) as Link` (unchecked); `z.object({..}).parse(j)` checked |
| Rust | `#[derive(Serialize)]` then `serde_json::to_string(&l)` | `#[derive(Deserialize)]` then `serde_json::from_str::<Link>(s)?` |

Mechanism. Not traits: kio's checker already knows every struct's shape, which is how `==`, display and `assert_eq` work on structs without a derive. The same knowledge gives `x.to_json()` on any struct or enum built from data types, and a decode generic in the result type, spelled as `empty::<T>()` is today (language.md "cannot infer `T`"): `Json::decode::<Link>(j) -> Result<Link, Str>` with the error text every app wrote by hand (`missing string field "amount" in data.object`). Generics alone do not give it (roadmap.md:95); more bounds would not either: a `T: ToJson` bound is only useful once `to_json` exists on structs, and then `page_json<T>` needs no bound at all. Open design points, for later: `Option` fields and absent keys, enums (tag or string), `Int` versus `Float`, renaming, unknown keys. It changes what programs rely on (the name mapping), so it is the maintainer's door, but it adds no syntax.

## Case 2: a SQL row to a struct

```
// flow/flow.kio:44-50 and payhook/app.kio:135-141, verbatim twice; sqlnotes/app.kio:57-65 the same as `text`/`row_note`
fn text_of(v: SqlValue) -> Str { match v { SqlValue::Text(s) => s, _ => "" } }
fn int_of(v: SqlValue) -> Int { match v { SqlValue::Int(n) => n, _ => 0 } }
// flow.kio:82                 Some(Job { id: id, body: text_of(rs[0][0]), attempts: seen + 1 })
// flow.kio:117, six columns   rs.map(|r| text_of(r[0]) + "|" + text_of(r[1]) + "|" + int_of(r[2]).to_str() + ..)
// tests.kio:222-227           struct M { id, state, attempts, lease, until, next } and `mline`, the same six again
```

| Occurrence | Cost |
|---|---|
| `text_of`/`int_of`: flow, payhook (7 lines each); sqlnotes `text`, `row_note` (9); primer.md:166 `r[0].as_str().unwrap_or("")` | A2 1.5 ("every Db program will write these", ~20 lines), A3 `int_at`/`text_at` |
| flow's `jobs` columns spelled in the `SELECT`, the `\|` join, `M` and `mline`: 4 places | F1.1 friction 11: rows joined as text "since there is no `format!` and no way to compare rows readably" |
| a wrong index or a `NULL` is silently `0` or `""` (flow.kio:45, 49) | a test can pass on a column shift; not a lost check (SQL text is opaque to the checker either way) but a lost loud failure |

Already solved: L31a's `as_int()`/`as_str()` make `text_of` a call (`v.as_str().unwrap_or("")`), and structs display and compare, so friction 11 was an existing feature unused (`jobs(db) -> List<Job>` and `assert_eq` on it). Left: one line per column with an index, in every row-reading function.

| | Row to struct |
|---|---|
| Python | `sqlite3.Row` by name, `Job(**row)` |
| Go | `rows.Scan(&j.ID, &j.Body, &j.Attempts)`: positional too; sqlx adds `db:"id"` tags and `StructScan` |
| TypeScript | `row as Job` (unchecked) |
| Rust | `#[derive(sqlx::FromRow)]` then `query_as::<_, Job>(sql)` by name; rusqlite `row.get(0)?` positional |

Mechanism: case 1's decode applied by position (`Row::decode::<Job>(r)`: field order is column order, count and type checked at run time, an `Err` naming the column). Go lives with positional `Scan`; the gain over L31a is modest, so this case adds weight to case 1 and does not stand alone.

## Case 3: handlers as values in a route table

```
// notes/app.kio:13-33 (kvnotes, sqlnotes, links 46-88, payhook 48-62, ws 9-26, flow app.kio 14-21 the same)
enum Act { Create, List, Show, Replace, Delete }
fn routes() -> List<web::Route<Act>> { [web::post("/notes", Act::Create), web::get("/notes", Act::List), ..] }
fn handle(r: web::Request, notes: Dir) -> web::Response {
    let m = match web::dispatch(routes(), r) { Some(m) => m, None => return web::not_found() };
    match m.action { Act::Create => create(r, notes), Act::List => list(r, notes), .. }
}
```

Impossible today by rule 1: `Route { handler: fn(Request, Dir) -> Response }` is "function values cannot take or return capabilities" and a closure cannot capture `notes` (language.md:250-252); a capability struct does not change this (`fn(Request, Env)` is refused too). kio-web's README:78 documents the workaround and why.

| Cost | Count |
|---|---|
| an `enum`, a `routes()` and a `match`: ~12 lines for 5 routes against 5 `HandleFunc` lines | 7 of 8 apps, ~7 extra lines each |
| a one-route app writes `enum Act { Ingest }` and `Some(_) =>` (F1.1 friction 10) | 1; `web::route(r, "POST", "/events")` already serves it (hello/app.kio:6) |
| checks lost | none: the `match` is exhaustive, so a new variant without a handler is a check error |

| | Route table |
|---|---|
| Python | `@app.post("/notes") def create(): ..` (Flask/FastAPI), deps as globals or `Depends` |
| Go | `mux.HandleFunc("POST /notes", s.create)`, `s` a struct holding the `*sql.DB` |
| TypeScript | `app.post("/notes", (req, res) => create(db, req, res))`, `db` captured |
| Rust | `Router::new().route("/notes", post(create)).with_state(state)`; `State<AppState>` extractor |

Every other language stores the dependency in a closure, a receiver or a state object: exactly what rule 1 forbids so that `list : notes list read` can be read off a signature. abstraction.md's option B (a `fn` type naming capabilities) would make a table possible at the price of a report line "any function of this type used as a value". The 7 occurrences are one source: the examples follow one README recipe, and neither held-out app (A2, A3) logged routing as friction. Verdict: not a case for a language change; the enum is the price of the exact report, and it is small.

## Not cases

| Pattern | Seen | Answer |
|---|---|---|
| `?` without `From`: `let n = match api::int_param(id, "note id") { Ok(n) => n, Err(e) => return e };` | notes 10 ladders, kvnotes 10, sqlnotes 6, payhook 8 (3-4 lines each); primer.md:70-83 | an idiom: links returns `Result<web::Response, web::Response>` and uses `?` and `map_err(storage_error)` (139); web-support-todo.md:302 already notes it. `From` is a trait for ~6 `map_err` lines: no |
| JSON object building, 5 lines for two fields | 12 `Map::new()` builders in kio-web; A3 F5; F1.1 friction 7 | solved by L31a `Json::obj().with(..)`; migrate kio-web |
| two-key sort `sort_by(\|a, b\| a.hits > b.hits \|\| (a.hits == b.hits && a.slug < b.slug))` | links:377, primer.md:37 | one line in Go and TypeScript too; `Ord` on structs would be case 1's family, later |
| `for (k, v) in m`: `for k in m.keys() { m.get(k).unwrap_or(0) }` | primer.md:14, links:173-184 | a library `entries()` returning a generic struct; no language change |
| methods on user types (`m.param("id")` for `web::param(m, "id")`) | everywhere | readability only; no check lost; `impl` stays out |
| work after the response: `struct Answer { res, hit, made }` | payhook:42, links:30 | an idiom, and a good one |
| rows as `\|`-joined text for `assert_eq` | flow.kio:41, 117; tests.kio:225 | structs display and compare already (F1.1 friction 11 answered) |
| grouping capabilities | A2 3.7, A3 F15 | landed, L32 |

## Recommendation

**No language change yet.** One mechanism has the evidence (two surfaces, four independent sources, ~50 lines in links and ~40 in A3's booking.kio, field names spelled 3 to 5 times each, checks that do not exist); nothing else does.

1. **Typed encode and decode of data structs** (cases 1 and 2): write the design note only after one more held-out app (A4 or F1.3) is built on L31a's `Json::obj().with` and `get_*`, so the measured residue is the derive's gain and not the builder's. Record now, for that note: derive-like from the checker's knowledge of struct shapes, no traits; a generic-in-the-result built-in as `empty::<T>()`; error texts as the apps wrote them; `Option` fields, enums, `Int`/`Float`, renaming and unknown keys to decide. Acceptance to aim at: links' JSON glue from ~50 lines to under 10 with no field name written twice; `api::page_json<T>` writable. A maintainer door (programs will rely on the mapping), not a syntax change.
2. **Handlers as values**: keep the enum. Revisit only if a held-out app logs it.
3. **Now, without the maintainer**: migrate kio-web to `Json::obj().with`, `get_str`/`get_int`, `SqlValue.as_*` and `to_str()` on errors; document the `Result<Response, Response>` idiom and `web::route` for one-route apps in kio-web's README; flow's `rows`/`jobs` as `List<Job>` compared with `assert_eq`. Then re-count: whatever glue is left after this is case 1's real size.
