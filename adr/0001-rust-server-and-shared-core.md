# 0001. Rust for the server and a shared core

- **Date:** 2026-09-23 11:23 -0600
- **Status:** Accepted
- **Commit:** 52e600e

## Context
Offline parity needs search, sort, and shuffle to behave identically on the server and on devices. The Raspberry Pi target also needs a small memory footprint with no GC pauses.

## Decision
- Rust on the stable toolchain, edition 2024, as one Cargo workspace under `crates/`.
- A pure `core` crate (no I/O, clock, or platform APIs) holds every rule that must match across platforms. It compiles natively for the server and to `wasm32-unknown-unknown` for the web client, through `core-wasm`.

## Consequences
- One implementation of each rule, instead of one per client.
- We pay for it in slower compiles and iteration.
