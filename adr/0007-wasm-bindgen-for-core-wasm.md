# 0007. wasm-bindgen for the core's WebAssembly bindings

- **Date:** 2026-10-27 21:00 -0600
- **Status:** Proposed
- **Commit:** (when it merges)

## Context
The web client runs the shared core's queue in the browser (ADR 0001), so `core-wasm` must expose it to JavaScript. The workspace forbids `unsafe`.

## Decision
`crates/core-wasm` is a `cdylib` using `wasm-bindgen`, with the `wasm-bindgen` CLI at the same exact version generating the JavaScript module during the web build. Structured values cross as JSON strings in the wire's shapes.

## Alternatives considered
- **Raw `extern "C"` exports.** Need `#[unsafe(no_mangle)]` and pointer handling by hand, which the workspace forbids.
- **`serde-wasm-bindgen`.** Passes objects without a JSON string, but adds a dependency for no gain while the queue's calls are human-rate.

## Consequences
- Building the web app needs the `wasm32-unknown-unknown` target and the matching `wasm-bindgen` CLI (`web/scripts/wasm.sh` says how to install it), and the development image builds the module in a Rust stage.
- Changing the `wasm-bindgen` version means changing it in `Cargo.toml`, the script, the Dockerfile, the CI workflow and `CONTRIBUTING.md` together.
