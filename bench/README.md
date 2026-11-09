# Benchmarks

kio compiled against the native interpreter, Rust (`-O`) and Python 3 on the same algorithms and sizes (C8a). Results and analysis: [docs/roadmap.md](../docs/roadmap.md), "Speed today".

```
KIO_WEB=path/to/kio-web bench/run.sh      # builds, writes inputs, prints the table (~3 min)
RUNS=1 ONLY=fib,tree NO_INTERP=1 bench/run.sh   # a quick subset
```

Needs Rust, Python 3, `ab` (ApacheBench, on macOS by default) and a kio-web checkout (default `../kio-web` beside this repo) for `json.kio` and the HTTP example.

| Path | What |
|---|---|
| `kio/` | the kio programs; each prints one number, which every version must match |
| `rust/` | the Rust versions, one binary each (a cargo project outside the workspace) |
| `py/` | the Python versions |
| `interp/` | `kio-interp`: the native interpreter (`kio_host::Interp`, the test oracle, not shipped) as a command |
| `gen_data.py` | writes `data/words.txt` and `data/data.json` (1 MB each, deterministic) |
| `run.py` | times everything and prints the table |
| `profile.py` | a `sample` profile of one compiled run, naming compiled frames |

How times are taken:
- Wall time of a whole process, best of `RUNS` (5) after one discarded run. Each column's startup (an empty program, `python3 -c pass`) is subtracted, so a time is the work alone.
- `kio compiled` runs from the compile cache; `compile` is a run with an empty cache minus a cached one, both with `--fuel 1` (stops at once). Runs use `--fuel 10^15 --timeout 3600 --memory 4096`: no benchmark stops at a limit (the default `kio run` fuel is 10 billion).
- `kio interpreted` is `lang::run` natively, without fuel or memory limits. A run over 10 s is timed once.
- HTTP: `ab -k -c 12 -n 50000` against kio-web's hello example under `--listen`, a hyper server and a `http.server` one answering the same route.

Profiling (macOS):

```
cd bench/build/kio && python3 ../../profile.py --secs 2 tree.kio --fuel 1000000000000000
```

`sample` cannot name JIT code. `profile.py` fills a private compile cache, finds the native code's region with `vmmap`, and names addresses from the cache entry's ELF symbols (`objdump -t`): runtime functions by name, program functions by `kio-interp --names` (codegen emits them in IR order after the runtime's). `sample` stops the process for about 0.3 s as it attaches, some tens of ms into the run; those samples land on one instruction and are dropped, so the profile covers the run after the attach. Use runs of 0.5 s or more.

Fuel and epoch checks cannot be turned off from the command line; their cost was measured with a scratch build of `host/src/config.rs` with `consume_fuel(false)` (and `epoch_interruption(false)`), run with `KIO=path/to/that/kio`.
