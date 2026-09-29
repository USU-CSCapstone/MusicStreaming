#!/bin/sh
# Builds the three test plugins, then the harness. Run from spikes/plugins.
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

echo "guest-rust (wasm32-wasip2)"
cargo build -q --release -p guest-rust --target wasm32-wasip2

echo "guest-js (componentize-js, interpreted and AOT)"
(cd guest-js && npm install --silent && npm run -s build)

echo "guest-process (native)"
cargo build -q --release -p guest-process

echo "lrclib-lyrics (a real plugin), packed into target/lrclib-lyrics.wasm"
cargo build -q --release -p lrclib-lyrics --target wasm32-wasip2
node ../../tools/plugin-pack/pack.mjs target/wasm32-wasip2/release/lrclib_lyrics.wasm \
	lrclib-lyrics/manifest.json -o target/lrclib-lyrics.wasm

echo "plugin-run (runs installed plugins)"
cargo build -q --release -p plugin-runner

echo "plugin-spike (host)"
cargo build -q --release -p plugin-spike

echo "done: ./target/release/plugin-spike all, or install target/lrclib-lyrics.wasm on the Plugins page"
