#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/listenbrainz.wasm, ready to install on
# the Plugins page. Cargo's output is read from $CARGO_TARGET_DIR when it is set.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
mkdir -p target
node ../../tools/plugin-pack/pack.mjs "${CARGO_TARGET_DIR:-target}/wasm32-wasip2/release/listenbrainz.wasm" \
	manifest.json -o target/listenbrainz.wasm
echo "packed target/listenbrainz.wasm"
