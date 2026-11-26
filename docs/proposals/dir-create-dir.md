# `Dir.create_dir`: one new directory

Proposal only; caps.wit is the maintainer's. Context: A5 wanted `out.write("2026-11-26/digest.md", ..)`
and got `NotFound` (a5-findings.md:31; guide.md:77 documents "a `Dir` has no `mkdir`"; status.md:170
recommends `create_dir`). Note: `Dir` has no `rename` today (caps.wit:11-19); the conventions matched below
are `create`, `remove`, `write` and `append`.

## The function

```
fn create_dir(self, path: Str) -> Result<(), IoErr>     // in kio: out.create_dir("2026-11-26")?
```

caps.wit, appended after `append` as H34 appended `append` (guest-host-todo.md:225), so no method index moves:

```
     append: func(path: string, data: list<u8>) -> result<_, io-err>;     // create or append, one write per call
+    create-dir: func(path: string) -> result<_, io-err>;                 // one new directory; the parent must exist
```

No new `io-err` variant: `exists`, `not-found`, `invalid-path` and `denied` cover every case below.

## Decisions

| Question | Recommendation | Alternatives | Reason |
|---|---|---|---|
| Levels | One level, one function. A5's case is `create_dir("2026-11-26")` then `write("2026-11-26/digest.md", ..)`; deeper trees are a loop over prefixes | `create_dir_all` as a second function; a `recursive: Bool` flag | `create`, `write` and `append` never create parents (guide.md:77); recursive creation must succeed on an existing directory, so it loses `exists`, and one call would do an unbounded number of operations behind one audit line and one `--calls` tick |
| `path` names a directory, file, symlink (any target, dangling too) or FIFO | `Err(Exists)`; nothing is changed or followed | `Ok(())` when a directory exists (idempotent) | `create` is "a new file only" (caps.wit:13) and returns `exists` (lib.rs:1930); an existing directory may be another run's and the program should know |
| Missing parent, or a parent that is a file | `Err(NotFound)` | a new `not-a-directory` variant | as `create("2026/11/a", ..)` today: `io_err` maps `NotFound` and `NotADirectory` to `not-found` (lib.rs:1319) |
| `..`, absolute path, a symlink at any component leading out, empty, NUL, over 4,096 bytes | `Err(InvalidPath)` | | `check_path` (lib.rs:1328-1333) and cap-std's escape error (lib.rs:1320), as every method |
| Trailing `/` | `Err(Denied)` | strip it | `write`, `append` and `remove` refuse `a/` as `denied` (lib.rs:1694, 1837-1839, 1878), never letting the kernel read a trailing slash, which macOS and Linux treat differently on a symlink |
| TOCTOU against a symlink swap | No check-then-act: one `dir.create_dir(path)` (cap-std), which resolves the parent in its own walk and issues `mkdirat` on that fd. `mkdir` never follows a symlink at the last name (main.rs:2626-2627 relies on this), so a swap between the walk and the `mkdirat` is `EEXIST` -> `Exists`; a swap of a middle component is resolved by cap-std inside the root or refused as an escape (design.md:87) | `kind()` first, then create | `create` has no precheck either: `create_new` "fails if the path exists, in the same call that creates it" (lib.rs:1925-1927); `write` and `append` precheck only because their final operation would follow a link |
| Quota | 0 bytes to `--io`; one call to `--calls` (`count_call`, lib.rs:1464). No entry quota exists (guide.md:511-522); `--calls` (10,000 a run) bounds creation attempts per run, not directories kept across runs, which use inodes and metadata no kio limit counts; the guide says so and never calls it a disk bound | charge `path.len()` as `list` charges names (lib.rs:1815) | `create` charges `data.len()` only (lib.rs:1913-1914), never the path; a directory has no data |
| `:ro` | `Err(Denied)` before the work, as `create` (lib.rs:1670-1673); the startup refusal lists it: lib.rs:2741 `["write", "create", "append", "remove"]` gains `"create_dir"`, review.rs:37 ("what is written or removed") too; `read_only`'s comment (lib.rs:1584-1585) names it | | H27: a `:ro` grant is enforced twice (design.md:87) |
| Authority report | method `create_dir`; `COVERS` row `("Dir", "create_dir", &[])`, standalone as `remove` is (report.rs:520-526) | covered by `write` | no existing method can make a directory, so it is new authority; `kio check --base` shows `~ out: Dir (+create_dir)` |

Error mapping in the host: `AlreadyExists` -> `Exists` (matched explicitly, as lib.rs:1930; `io_err` would say `denied`),
everything else through `io_err` (lib.rs:1317-1323).

## Audit

One line per call through `call` (lib.rs:1454-1489), the format of lib.rs:1491-1517, `bytes` 0, `result` as `outcome` spells
the variant (lib.rs:1433-1446):

```
{"event":"call","seq":4,"cap":"out","kind":"Dir","method":"create_dir","arg":"2026-11-26","bytes":0,"result":"ok","ms":0.1}
{"event":"call","seq":5,"cap":"out","kind":"Dir","method":"create_dir","arg":"2026-11-26","bytes":0,"result":"exists","ms":0.0}
```

A `:ro` refusal is `"result":"denied"`, as `create`'s (audit.rs:127 `refused_create_is_logged`).

## Test host, interpreter, compiled

- `kio test` has no fake `Dir`: a `d: Dir` parameter is a fresh `TempDir` (main.rs:2625-2640; guide.md:691), a property one per case
  (guide.md:779). `create_dir` works there as on any grant; teardown is `remove_dir_all`, which never follows a symlink (main.rs:2627).
  An `--http-record hook=sent` record is a new file `hook-N` (guide.md:774): a directory the program made at that name fails the record
  as a file does today ("cannot write `hook-2`: File exists"). Nothing to add.
- Interpreter: host/src/interp.rs:314-317 gains `("Dir", "create_dir", [Value::Str(p)])` calling `HostDir::create_dir`, the one host
  function. Compiled: runtime/src/caps.rs:248-252 gains `rt_cap_dir_create_dir(ret, h, path)` calling `d.create_dir(path)` (wit-bindgen
  from caps.wit, entry.rs:10); codegen derives the import name from the caps table (codegen/src/lib.rs:3450); the checker's table is
  `cargo run -p capgen`. Both paths reach `State::create_dir` in lib.rs, so one implementation, tested on both (dir_append.rs:56 pattern).

## Hints

- Checker: `d.mkdir(..)`, `d.make_dir(..)`, `d.create_dir_all(..)` already get "no method `mkdir` on Dir" with "Dir has: .." (check.rs:3601-3603);
  the list gains `create_dir` from the generated table. Add, for those three names: "use `create_dir`, one level at a time".
- Runtime `Err` values print as today (`error: IoErr::Exists`); `IoErr` row in language.md:159 needs no change.
- Startup: "`out` is granted `:ro` but the program can write it (`out create_dir`)", hint "drop `:ro`, or remove the write" (lib.rs:1168), unchanged.

## Done when

- host/tests/dir.rs: `create_dir_then_write_and_list` (`sub/` listed, `exists("sub")` true); `create_dir_on_an_existing_name_is_exists`
  (directory, file, symlink to a file, dangling symlink, symlink to a directory outside the root: all `Exists`, nothing created outside,
  checked as lines 209-211 check); `create_dir_without_a_parent_is_not_found` (`a/b`, and `f/b` with `f` a file); `create_dir` added to
  the `bad` loop and the empty/NUL loop of `escapes_are_invalid_path` (lines 196-207); `a/` is `Denied`; `report_lists_the_new_methods`
  (line 215) gains it.
- host/tests/dir_ro.rs:47 and :89: `create_dir` denied on both paths and refused at start (`out create_dir`).
- host/tests/audit.rs:190 `dir_methods_are_logged`: a `create_dir` line with `bytes` 0, and an `exists` result.
- host/tests/compiled_caps.rs:118 or a `dir_create_dir.rs` as dir_append.rs:26: interpreter and compiled agree on every outcome above;
  `--calls 3` stops the fourth `create_dir`; `--io` is unchanged by a thousand of them.
- host/tests/dir.rs, the existing adversarial pattern: a thread swaps the parent (and a middle component) between a directory and a symlink out of the root while `create_dir` runs in a loop; nothing is ever created outside the root.
- lang/tests/check.rs: the `mkdir` hint; `cargo run -p capgen` leaves `caps_gen.rs` unchanged (CI).
- Docs: caps.wit comment; language.md:202 table; guide.md:77 rewritten ("`create_dir` makes one level; parents first"); design.md:87
  (`:ro` list, "only regular files are .. created"); report.rs COVERS comment; status.md:170 row closed; README:189 unchanged.

## Open for the maintainer

1. `Exists` on an existing directory (recommended, matches `create`) or `Ok(())` (idempotent, as `mkdir -p`'s last level; loses the signal).
2. Whether `remove` should learn to remove an empty directory in the same change, or stay "one regular file" (recommended: stay; a second
   caps.wit change when an app needs it).
3. `COVERS`: standalone (recommended) or covered by `write`, which decides whether adding `create_dir` to a program that already writes
   shows up in `kio check --base` and `kio review`.
