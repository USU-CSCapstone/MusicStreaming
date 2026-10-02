#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/listenbrainz.wasm, ready to install on
# the Plugins page.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
node ../../tools/plugin-pack/pack.mjs target/wasm32-wasip2/release/listenbrainz.wasm \
	manifest.json -o target/listenbrainz.wasm
echo "packed target/listenbrainz.wasm"
