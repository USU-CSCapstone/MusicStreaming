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

echo "plugin-spike (host)"
cargo build -q --release -p plugin-spike

echo "done: ./target/release/plugin-spike all"
