#!/bin/sh
# Build kio, the interpreter harness and the Rust benchmarks (release), write the inputs, then
# time everything and print the table (bench/README.md). KIO_WEB: a kio-web checkout
# (default ../kio-web beside this repo); RUNS, ONLY, NO_INTERP: see bench/run.py.
set -e
cd "$(dirname "$0")"
BENCH=$(pwd)
ROOT=$(dirname "$BENCH")
KIO_WEB=${KIO_WEB:-$ROOT/../kio-web}
[ -f "$KIO_WEB/json.kio" ] || { echo "kio-web not found at $KIO_WEB: set KIO_WEB"; exit 1; }
command -v ab >/dev/null || { echo "ab (ApacheBench) not found"; exit 1; }

(cd "$ROOT" && cargo build --release -p kio)
CARGO_TARGET_DIR="$BENCH/target" cargo build --release --manifest-path interp/Cargo.toml
CARGO_TARGET_DIR="$BENCH/target" cargo build --release --manifest-path rust/Cargo.toml

python3 gen_data.py
rm -rf build
mkdir -p build/kio build/hello
cp kio/*.kio build/kio/
# `Json` is a built-in type now (L9); until kio-web moves to it, its json.kio's own enum is renamed.
sed 's/Json/JsonKio/g' "$KIO_WEB/json.kio" > build/kio/json.kio
cp -R "$KIO_WEB/examples/hello/app.kio" "$KIO_WEB/examples/hello/public" "$KIO_WEB/web.kio" build/hello/
exec python3 run.py
