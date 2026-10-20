#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/lrclib-lyrics.wasm, ready to install on
# the Plugins page. Cargo's output is read from $CARGO_TARGET_DIR when it is set.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
mkdir -p target
node ../../tools/plugin-pack/pack.mjs "${CARGO_TARGET_DIR:-target}/wasm32-wasip2/release/lrclib_lyrics.wasm" \
	manifest.json -o target/lrclib-lyrics.wasm
echo "packed target/lrclib-lyrics.wasm"
