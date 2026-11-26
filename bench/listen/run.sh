#!/bin/sh
# C24: `--listen` per request, kio against Rust (hyper) twins, the in-process probe and (Linux)
# a perf split. Builds release kio, the twins and the probe, then runs bench.py; everything goes
# to OUT (default bench/listen/out-<time>), the summary to OUT/report.md.
#
#   bench/listen/run.sh                                  # full: 5 rounds x 50k requests, c=12
#   ROUNDS=3 N=3000 WARM=500 PROBE_N=2000 PERF_SECS=3 bench/listen/run.sh   # smoke
#
# Needs: cargo, python3, oha (`cargo install oha --version 1.17.0 --locked`; or OHA=path),
# and on Linux `perf` (linux-perf / linux-tools) with kernel.perf_event_paranoid=-1 for the split.
# TOOLCHAIN=1.100.0 runs `cargo +1.100.0` (a rustup override that pins another one); CARGO_TARGET_DIR
# is honoured (default: the workspace's target/). NO_PERF=1 skips the frame-pointer build and perf.
set -e
cd "$(dirname "$0")"
HERE=$(pwd)
ROOT=$(cd ../.. && pwd)
PLUS=${TOOLCHAIN:+"+$TOOLCHAIN"}
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
OUT=${OUT:-$HERE/out-$(date +%Y%m%d-%H%M%S)}
OHA=${OHA:-$(command -v oha || true)}
[ -x "$OHA" ] || { echo "oha not found: cargo install oha --version 1.17.0 --locked, or set OHA"; exit 1; }
mkdir -p "$OUT"

export CARGO_TARGET_DIR="$TARGET"
(cd "$ROOT" && cargo $PLUS build --release -p kio)
cargo $PLUS build --release --locked --manifest-path rust/Cargo.toml
# The probe depends on host by path: a version bump changes its lock, so it is not --locked
# (its other pins stay as the lock has them).
cargo $PLUS build --release --manifest-path probe/Cargo.toml
KIO_FP=
if [ "$(uname -s)" = Linux ] && [ -z "$NO_PERF" ] && command -v perf >/dev/null; then
  # The split needs whole stacks: the same source with frame pointers (Cranelift's code keeps them).
  (cd "$ROOT" && CARGO_TARGET_DIR="$TARGET/fp" RUSTFLAGS="-C force-frame-pointers=yes" cargo $PLUS build --release -p kio)
  KIO_FP="$TARGET/fp/release/kio"
fi

KIO="$TARGET/release/kio" KIO_FP="$KIO_FP" TWINS="$TARGET/release" PROBE="$TARGET/release/kio-listen-probe" \
  OHA="$OHA" OUT="$OUT" ROOT="$ROOT" RUSTC_VERSION="$(rustc $PLUS -V)" \
  python3 bench.py
