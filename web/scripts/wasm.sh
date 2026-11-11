#!/bin/sh
# Builds crates/core-wasm and crates/catalog-wasm and binds them for the web app into
# src/lib/core-wasm and src/lib/catalog-wasm (gitignored).
# `pnpm dev`, `build`, `check` and `test:unit` run it first. Needs the `wasm-bindgen` CLI at
# exactly the version in the crates' Cargo.toml (also named in the Dockerfile):
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked
set -eu

version=0.2.129
root=$(cd "$(dirname "$0")/../.." && pwd)

if [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $version" ]; then
	echo "wasm-bindgen $version is required: cargo install wasm-bindgen-cli --version $version --locked" >&2
	exit 1
fi

target=${CARGO_TARGET_DIR:-$root/target}

cd "$root"
# Builds into the directory bound below, whatever target-dir Cargo is configured with.
cargo build --quiet --release --target wasm32-unknown-unknown \
	--package jewelcase-core-wasm --package jewelcase-catalog-wasm --target-dir "$target"
for module in core catalog; do
	wasm-bindgen --target web --out-dir "web/src/lib/$module-wasm" \
		"$target/wasm32-unknown-unknown/release/jewelcase_${module}_wasm.wasm"
done
