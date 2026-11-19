# Standard library: what agent-written apps rewrite (2026-11-19)

Sources: A2 FRICTION.md (inventory, 0.1.3) and A3 FRICTION.md (booking, 0.1.5), plus their *.kio; F1.1/F1.2 friction (product-todo.md 213, 221); the E2 gaps table (lang-todo.md 330); roadmap.md "What a real app needs"; a2/a3-findings.md; kio-web (web.kio 280, api.kio 92, http.kio 71 lines; 8 examples). Already counted as done or pinned: L29 (checked arithmetic, pow/abs, bitwise methods, url_encode/decode, iso_utc/parse_iso_utc, test unwrap), `Secret`, append on `Dir`, `try_to_str`, `from_hex`.

Evidence key: **A2**, **A3** = held-out apps; **F1** = flow (kio-web); **KW** = kio-web library and examples; **E2** = eval references; **RM** = roadmap only; **ext** = Python/Go stdlib usage.

## Ranked

| # | Item | Evidence (hits, workaround) | Form | Proposed API | Cost |
|---|---|---|---|---|---|
| 1 | **Dates**: parse, format, calendar | A2 4.9 (dates.kio 38 lines, Hinnant), A3 F2 (timeutil.kio 116 lines, its ranked #2), A2 #10, RM; ext: `datetime`/`time` is in nearly every service | B | **L29 covers most of it.** What's left: `Int.date_utc() -> Str` (`YYYY-MM-DD`; both apps wrote it, today `iso_utc().slice(0,10)`) and `Int.add_days`/weekday only once an app needs them | S (left) |
| 2 | **JSON object building and typed field reads** | A3 F5 (~40 lines of `insert`, ~25% of booking.kio; `str_field`, `int_field`, `int_at`), A2 (`field_str`, `field_int`, `object`, `only_fields`, `msg_json`, `num`), F1.1 ("no JSON object builder"), KW (`api::str_field` 8 uses, `api::error` 39, `int_field`, `api::json`); 55 `insert`/`Json::Obj` lines across A2+A3 | B (methods), later L | `Json::obj() -> Json`, `j.with(Str, Json) -> Json` (chainable); `j.get_str(Str) -> Option<Str>`, `get_int`, `get_bool`, `get_float`, `get_list` | S |
| 3 | **Query strings and form decoding** | A2 1.7 (worked around by restricting skus), A3 F3 (`pct_decode` + `hexval` + `query_param`, 38 lines), KW (`web::query`, `decode`, `hex`, `digit`, 50 lines) | B (L29) + M | L29's `url_decode` keeps `+` as `+`, but **all three hand-written decoders map `+` to space** (query strings are form-encoded). Add `Str.form_decode() -> Option<Str>` (`+` is space) or have `web::query` replace `+` first; M: `web::query` rebuilt on it | S |
| 4 | **Db row decoding** | A2 1.5 (`text`, `int`, `row_item`, ~20 lines, "every Db program will write these"), A3 (`int_at`, `text_at`), KW (`text_of`, `int_of` in 2 examples each), the primer ships "Db column helpers" | B | `SqlValue.as_int() -> Option<Int>`, `as_str`, `as_float`, `as_bytes`, `is_null() -> Bool` (as `Json`'s accessors) | S |
| 5 | **Header lookup** | A2 (`inv::header`), A3 F8 (`app::header`), KW (`web::header`, `http::header`), guide's own Secret example says "your own lookup": 4 independent copies | M now; C optional | `web::header(r, name)` already exists. Built-in: `Incoming.header(Str) -> Option<Str>` and `HttpResponse.header` (case-insensitive) would need caps.wit, or a lang-side method over the two lists (B, no wit change) | S |
| 6 | **Error enums to text** | A2 (`db_msg` 8 lines, `http_msg` 10), KW (`http::describe`, `io_error`, `storage_error` in 2 examples each), A3 `db_fail` drops the error; A2 3.3 asks TooLarge to name its limit | B | `DbErr.to_str()`, `HttpErr.to_str()`, `IoErr.to_str()`, `WebErr.to_str()`, `KvErr.to_str()` | S |
| 7 | **A run-time value for `main`** | A2 4.1 (config file, parser, 9 checks, 25 lines), A3 F31 (25 lines): 2 of 2 apps; RM (clap row) | C | `fn main(hours: Int, ..)` with `kio run --value hours=24`, shown in the report and audit header | M, maintainer |
| 8 | **CSV** | A2 stage 3 (csv.kio 56 lines; the byte rewrite introduced a `""` bug; parse was the fuel hot spot: 42,000 fuel a row before rewrite, import failed live at 10M fuel), ext: `csv` is a top-20 Python stdlib import for jobs | B | `Str.parse_csv() -> Result<List<List<Str>>, Str>` (RFC 4180, error names line and column), `List<Str>.to_csv_line() -> Str` | M |
| 9 | **Checked / wide arithmetic** | A2 4.8 (12 lines + property), E2 gaps, F1.2 (no pow/shift) | B | **L29** | done when L29 lands |
| 10 | **Sleep / backoff** | A2 4.7 (no backoff), F1.2 (worker is one pass), A3 worked around with a Db outbox | C | `Clock.sleep_ms(Int)`, bounded by `--timeout`, audited; `fixed:` clocks advance instead of waiting | M, maintainer (status: waits for a written requirement) |
| 11 | **Zero-padding / formatting** | A2 `dates::pad`, A3 `timeutil::pad` (same 5 lines), F1.1 "no `format!`" | B | `Str.pad_start(Int, Str) -> Str` (`"7".pad_start(2, "0")`); `format!` is L and not needed yet | S |
| 12 | **Rows from a write** | A3 F4 (`RETURNING` through `query`, reported as `query`), F29 (transaction returns only a count; extra lookup), F1.2 (claim is SELECT + UPDATE + SELECT) | C | `Db.exec_returning(Str, List<SqlValue>) -> Result<List<List<SqlValue>>, DbErr>`, or `transaction` returning each statement's rows | M, maintainer |
| 13 | **Base64 / base64url** | 0 app hits (both apps used hex); ext: `base64` in most Python services (Basic auth, JWT, data URLs, webhook signatures from Stripe-style hosts are hex but many are base64) | B | `Bytes.to_base64() -> Str`, `Str.from_base64() -> Option<Bytes>`, `_url` variants | S |
| 14 | **Text validation / regex** | A2 (`name_byte`, `all_name_bytes`, `has_control`, `valid_sku`, `valid_file`: byte loops written for fuel), A3 `valid_email`; RM "No regex"; ext: `re` is a top-3 Python stdlib import | B | Near term `Str.is_ascii_alnum()`-class checks are not worth it; a small linear-time regex: `Str.matches(Str pattern) -> Bool`, `Str.captures(Str) -> Option<List<Str>>` (pattern errors at check time for a literal) | L |
| 15 | **Grouping capabilities** | A2 3.7 (~25 signatures, ~60 call sites by sed), A3 F15 (its #1; ~40 lines by script) | L | a struct of capabilities, reported per field | L, maintainer |

Below the cut (single source or no app hit): List `insert`/`remove` (E2; B, S); `Map.contains_key`, sets, `for` over a map (RM; B/L); UUID (`uuid::v4(rng)`, M, S: both apps used `rng.bytes(n).to_hex()` happily); log levels (RM); typed JSON decode (RM's top "what is left"; L, needs derive); `HttpErr` telling refused from denied (A3 F23; C).

Bugs found in kio-web's hand-written helpers (evidence that hand-written glue goes wrong, and fix-now items):
- `web::decode` ends in `out.to_str()`: a `%ff` in a query stops the run (500). Use `try_to_str`.
- `web::query` keeps a pair only when `split("=")` gives exactly 2 parts: `?q=a=b` and `?flag` are silently dropped.
- `web::json_str` escapes only `"` and `\`: a newline or control byte produces invalid JSON. `Json::Str(s).to_json()` already escapes correctly; replace it.

## Recommended first batch (after L29)

All built-in methods, no syntax, no caps.wit; each S; together they remove ~70% of the helper lines both apps wrote that L29 does not already remove.

1. `SqlValue.as_int/as_str/as_float/as_bytes/is_null` (#4)
2. `Json::obj()`, `Json.with`, `Json.get_str/get_int/get_bool/get_float/get_list` (#2)
3. `to_str()` on `DbErr`, `HttpErr`, `IoErr`, `WebErr`, `KvErr` (#6)
4. Form decoding: settle `+` in L29 now (`form_decode`, or a `url_decode` note) and fix kio-web's `query`/`decode`/`json_str` (#3, bugs above)
5. `Str.pad_start` (#11), `Bytes.to_base64`/`Str.from_base64` (#13)

Second batch: CSV built-in (#8, M), then regex (#14, L) when an app asks for it.

## Needs the maintainer

| Item | Kind | Recommendation |
|---|---|---|
| Value for `main` (#7) | C, caps.wit | Yes: 2 of 2 apps hit it; `--value name=..` for `Int`/`Str`/`Bool` params |
| Sleep (#10) | C, caps.wit | `Clock.sleep_ms` under `--timeout`; fixed clocks advance (also fixes A3 F26) |
| Rows from a write (#12) | C, caps.wit | rows from `transaction`; report a returning statement as a write |
| Header lookup as a method (#5) | C if on `Incoming` | optional; M already works |
| Grouping capabilities (#15) | L | design proposal; the largest recurring cost not in the library |
| Typed JSON decode | L | not yet; #2's getters cover the apps' needs |

## Built-in or kio-written module?

| | Built-in (B) | kio module via kio.deps (M) |
|---|---|---|
| Speed | Rust speed (Json 1.5x serde_json) and fuel-cheap | ~5x slower (json.kio vs Rust); byte loops cost ~29 fuel a byte, which is what broke A2's import under `--listen` fuel |
| Review | trusted runtime code; pure, so it adds no authority, but a bug is in every program | authority visible in the report; reviewable kio; a bug is the library's |
| Maintenance | interpreter + runtime + differential harness + kiogen per method (the L26/L27 pattern: ~1 commit each) | one kio file; versions pinned by sha256; no release of kio needed |
| Model fluency | in language.md, so models use it from the spec | needs the module's docs in the prompt |

Rule that fits the evidence: **B** for pure functions that run per byte or per row (dates, decoding, CSV, base64, regex) or that every program rewrites identically (row and Json accessors, error `to_str`); **M** for glue that encodes a convention (routing, request/response shapes, query maps, pagination, UUID); **C** only for things that touch time or the operator. Pure built-ins do not widen authority, so the trust cost is code size in the runtime, not reach.

## External signal

Weak: no published per-module import counts were found for Python or Go stdlib in backend code. One study of 129 popular Python repos found `os`, `sys`, `re` the most used stdlib modules; LLM-generated Python imports stdlib ~41-45% of the time, third-party ~48-54%, and models use a narrow set (32-39 distinct libraries across hundreds of tasks). From experience (not measured here), a Python/Go service's stdlib core is: json, datetime/time, re, urllib.parse / net/url, hashlib/hmac, base64, uuid, logging, csv, strconv/fmt formatting; kio has or has pinned json, time (L29), url (L29), hashing/HMAC, and lacks base64, regex, csv, formatting, levelled logging.

Sources: [Top used modules in Python OSS](https://www.academia.edu/113924352/What_are_the_Top_Used_Modules_in_Python_Open_Source_Projects); [LLM-generated library imports](https://arxiv.org/html/2507.10818v1); [LLMs' library preferences](https://arxiv.org/pdf/2503.17181); [ChatGPT as a software librarian](https://arxiv.org/pdf/2408.05128); [brettcannon/stdlib-stats](https://github.com/brettcannon/stdlib-stats).
