#!/bin/sh
# Builds crates/core-wasm and binds it for the web app into src/lib/core-wasm (gitignored).
# `pnpm dev`, `build`, `check` and `test:unit` run it first. Needs the `wasm-bindgen` CLI at
# exactly the version in crates/core-wasm/Cargo.toml (also named in the Dockerfile):
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked
set -eu

version=0.2.129
root=$(cd "$(dirname "$0")/../.." && pwd)

if [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $version" ]; then
	echo "wasm-bindgen $version is required: cargo install wasm-bindgen-cli --version $version --locked" >&2
	exit 1
fi

cd "$root"
cargo build --quiet --release --target wasm32-unknown-unknown --package jewelcase-core-wasm
wasm-bindgen --target web --out-dir web/src/lib/core-wasm \
	"${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-unknown/release/jewelcase_core_wasm.wasm"
