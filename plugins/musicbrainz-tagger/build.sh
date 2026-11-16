#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/musicbrainz-tagger.wasm, ready to install
# on the Plugins page. Cargo's output is read from $CARGO_TARGET_DIR when it is set.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
mkdir -p target
node ../../tools/plugin-pack/pack.mjs "${CARGO_TARGET_DIR:-target}/wasm32-wasip2/release/musicbrainz_tagger.wasm" \
	manifest.json -o target/musicbrainz-tagger.wasm
echo "packed target/musicbrainz-tagger.wasm"
