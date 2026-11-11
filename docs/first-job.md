# Your first job in 10 minutes

A kio program that reads a CSV of orders, POSTs each one to an API with a key it never sees, and writes a summary. You need a terminal and Python 3 (for the stand-in API); no Rust.

<!-- Every ```console block below runs in CI and must print what it shows: tools/first-job-test.py. -->

## 1. Download

macOS (Apple silicon):

```sh
curl -LO https://github.com/danglangs/kiolang/releases/latest/download/kio-aarch64-apple-darwin.tar.gz
curl -LO https://github.com/danglangs/kiolang/releases/latest/download/kio-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c kio-aarch64-apple-darwin.tar.gz.sha256
tar xzf kio-aarch64-apple-darwin.tar.gz && cd kio-aarch64-apple-darwin
```

Linux (x86_64, a static binary): the same with `kio-x86_64-unknown-linux-musl` and `sha256sum -c`. (The macOS binary is not notarized: download with `curl`, not a browser, or macOS will refuse to open it.) Then put `kio` on your `PATH` and say hello:

```console
$ export PATH="$PWD:$PATH"
$ kio run hello.kio
hello, kio
```

## 2. The program

```console
$ cd first-job
$ cat data/orders.csv
id,customer,cents
1001,acme,12000
1002,globex,4550
1003,initech,oops
1004,acme,30000
```

`push.kio` (open it: 34 lines) names what it needs as `main`'s parameters, and nothing else is reachable:

```rust
fn main(orders: Dir, api: Http, out: Dir) -> Result<Str, Failed>
```

`kio check` type-checks it and prints its authority: what each function can call on each grant.

```console
$ kio check push.kio
main    : orders read, api send POST, out write
parse   : none
to_json : none
```

## 3. The grants

The operator (you) says what each name is, in a file reviewed beside the code: `orders` is read-only, `api` is one local host that may receive POST, and the key comes from an environment variable.

```console
$ cat app.grants
# What push.kio may touch. Review this file beside the code.
--dir orders=data:ro
--dir out=out
--http api=http://127.0.0.1:8787
--http-methods api=POST
# The host adds the key to every request; the program never holds it.
--http-header api=Authorization:env:API_KEY
--audit audit.jsonl
```

`kio check --grants` binds the file to `main` without running anything (a CI step):

```console
$ kio check push.kio --grants app.grants
main    : orders read, api send POST, out write
parse   : none
to_json : none
```

## 4. Run it

Start the stand-in API (it answers 201 only with the right key), then run the job:

```console
$ mkdir out
$ export API_KEY='Bearer demo-key-123'
$ python3 api.py > api.log &
$ sleep 1
$ kio run push.kio --grants app.grants
main    : orders read, api send POST, out write
parse   : none
to_json : none
Ok("sent 3 orders, 46550 cents\nbad row: 1003,initech,oops")
$ cat out/summary.txt
sent 3 orders, 46550 cents
bad row: 1003,initech,oops
$ cat api.log
api: listening on http://127.0.0.1:8787
api: got order 1001 (acme, 12000 cents)
api: got order 1002 (globex, 4550 cents)
api: got order 1004 (acme, 30000 cents)
```

The bad row was skipped and reported; the three good orders arrived with the key.

## 5. The audit log

`audit.jsonl` has a line per call the program made, with what the host did:

```console
$ grep '"cap":"api"' audit.jsonl | head -1
{"event":"call","seq":2,"cap":"api","kind":"Http","method":"send","arg":"/orders","bytes":45,"result":"ok","timeout_ms":0,"http":"POST","headers":["content-type"],"injected":["authorization"],"sent":45,"status":201,"received":0,"ms":3.6}
$ grep -c demo-key audit.jsonl
0
```

`"injected":["authorization"]`: the host added the key. It is not in the log, and the program never held it.

## 6. Review an agent's edit

An agent changes the job to delete `orders.csv` after sending, so orders are not sent twice. Before reading the diff, ask what authority it adds:

```console
$ kio check push-v2.kio --base push.kio
~ orders: Dir (+remove)
authority added (exit 3); --detail lists each function
```

One line: `orders` gains `remove`. The grants file still says `data:ro`, so the edit is refused before it runs:

```console
$ kio run push-v2.kio --grants app.grants
kio: `orders` is granted `:ro` but the program can write it (`orders remove`)
  hint: drop `:ro`, or remove the write
$ kill %1
```

To allow it you would change `app.grants` (`--dir orders=data`), a one-line diff a reviewer sees. Next: [guide.md](guide.md) for every grant and flag, [primer.md](primer.md) for the language by example.
