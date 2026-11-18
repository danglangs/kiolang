# A2: what a newcomer hit building a second app (2026-11-17)

A fresh Opus agent built an inventory service (Db API, bearer auth and audit, CSV import and export, a nightly report job as a second entry) in four stages from the public docs and kio v0.1.3 alone: 34 tests pass, 51 friction entries. The app and its log stay local (~/kio-a2/app, FRICTION.md). Checked by hand: the `chars()` cost (a 100-character loop costs about 590 fuel a character, `contains` about 6), the text-mode diagnostics (no column, no argument named), the `^` and `[None, Some(..)]` hints.

Worked well: the first 190 lines checked and passed first time; each stage's authority diff was exactly that stage's requirement; `:ro` Db accepted for a job whose modules hold writes it never reaches; refusals and hints mostly excellent; 20,000-row import in about 40 ms.

| # | Shortfall | Cost to the builder | Task |
|---|---|---|---|
| 1 | `chars()` about 100x the cost of a byte scan; `Map.insert` about 2,900 fuel; no way to see where fuel goes | 30 min, 40 lines rewritten, a bug introduced | P9, T3 |
| 2 | Tests run with 10 billion fuel, `--listen` gives a request 10 million: an import passed every test and failed its first live run with an empty 500 | 15 min | T3 |
| 3 | No secret a server can compare an incoming token against | 20 min, workaround (a hash in a `:ro` dir) | one-way door |
| 4 | Text diagnostics drop the column and the argument (`tests.kio:7: error: expected Dir, found Str`); `^` says "unexpected character", not "no bitwise operators"; `[None, Some("")]` hint names `Option<Int>`; `\u` hint and assert output double-escape `\` | 10 min | L28 |
| 5 | No HTTP fake for tests: webhook tests need a real server running, and the review page labels them `tested` without naming it | 15 min | T4 |
| 6 | No plain value for `main` (a threshold): a config file, parser and 9 checks instead | 10 min, 25 lines | one-way door |
| 7 | No append on `Dir`: the audit log became a `Log` (`req N:` prefix, unreadable from tests) | 15 min | one-way door |
| 8 | Docs: `Incoming`/`Outgoing` missing from the test-defaults table; which characters generated `Str`s contain (no newline was ever generated); properties take about 60 ms a case, not 30; schema-at-each-request idiom unstated; `DbErr::TooLarge` names no limit | 25 min | D6, L28 |
| 9 | Review: the binds line does not show the grants (`:ro`, host, verbs); added tests' fuel is not shown; an entry's own tests are not run | 6 min | R1a5 |
| 10 | Library gaps: test `unwrap`, checked arithmetic, dates, URL decoding, sleep | 25 min | later; listed in product-todo |
