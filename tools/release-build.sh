#!/bin/sh
# Build a release archive and its SHA-256 file:
#   tools/release-build.sh [TARGET]   (default: the host's)
# writes dist/kio-TARGET.tar.gz{,.sha256}; no version in the name, so
# releases/latest/download/kio-TARGET.tar.gz is a stable link. The archive holds
# the `kio` binary (runtime.wasm is embedded by codegen's build.rs; nothing else
# is needed at run time), hello.kio and examples/first-job, for docs/first-job.md.
set -eu
cd "$(dirname "$0")/.."
target=${1:-$(rustc -vV | sed -n 's/^host: //p')}
name=kio-$target
out=${CARGO_TARGET_DIR:-target}/$target/release/kio

cargo build --release --locked -p kio --target "$target"

rm -rf "dist/$name" "dist/$name.tar.gz" "dist/$name.tar.gz.sha256"
mkdir -p "dist/$name"
cp "$out" LICENSE-APACHE LICENSE-MIT examples/tour/hello.kio "dist/$name/"
cp -R examples/first-job "dist/$name/first-job"
cd dist
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$name.tar.gz" "$name"
if command -v sha256sum >/dev/null; then
  sha256sum "$name.tar.gz" >"$name.tar.gz.sha256"
else
  shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256"
fi
cat "$name.tar.gz.sha256"
