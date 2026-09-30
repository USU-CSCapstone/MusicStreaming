#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/lrclib-lyrics.wasm, ready to install on
# the Plugins page.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
node ../../tools/plugin-pack/pack.mjs target/wasm32-wasip2/release/lrclib_lyrics.wasm \
	manifest.json -o target/lrclib-lyrics.wasm
echo "packed target/lrclib-lyrics.wasm"
