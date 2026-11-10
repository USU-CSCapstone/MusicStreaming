#!/usr/bin/env sh
# Builds the plugin and packs its manifest in: target/e2e-test-plugin.wasm, which the end-to-end
# tests install. Cargo's output is read from $CARGO_TARGET_DIR when it is set.
set -eu
cd "$(dirname "$0")"
cargo build -q --release --target wasm32-wasip2
mkdir -p target
node ../../../tools/plugin-pack/pack.mjs "${CARGO_TARGET_DIR:-target}/wasm32-wasip2/release/e2e_test_plugin.wasm" \
	manifest.json -o target/e2e-test-plugin.wasm
