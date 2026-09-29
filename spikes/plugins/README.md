# Plugin execution-model spike

Measures WebAssembly components hosted in-process by Wasmtime against an out-of-process
baseline, on the constraints in `requirements/plugins.md` §2 and §11. The findings and the
decision are in [`design/plugins.md`](../../design/plugins.md). This directory is the evidence.

It is its own Cargo workspace, so Wasmtime's compile stays out of the main build.

## Running it

Needs Rust with the `wasm32-wasip2` target (the root `rust-toolchain.toml` lists it) and Node.

```sh
./build.sh                                  # the three test plugins, then the harness
./target/release/plugin-spike all           # every experiment, as markdown tables
./target/release/plugin-spike data          # or one: calls instantiate data hang crash concurrency yield state
./target/release/plugin-spike all --json results/<machine>.json
cargo test --release -p plugin-spike        # quick versions of the asserted properties
```

`SPIKE_TICK_US` sets the epoch tick (default 250). The run exits non-zero if any containment
or scoping check fails. Performance bars are proposals, so they are reported, not asserted.

## Layout

| Path | |
|---|---|
| `wit/plugin.wit` | The interface under test: scoped library reads, host-side state, and exports that misbehave on purpose (`spin`, `crash`, `hog`) |
| `host/` | The harness. `wasm.rs` is the candidate host (epoch deadlines and yields, a memory limiter, a pause hook); `process.rs` is the baseline; `library.rs` is the scoped synthetic catalog; `main.rs` runs E1–E8 |
| `guest-rust/` | The test plugin in Rust, built with wit-bindgen |
| `guest-js/` | The same plugin in JavaScript, built with componentize-js, interpreted and AOT |
| `guest-process/` | The same plugin as a native process, speaking length-prefixed JSON over stdin/stdout |
| `results/` | Recorded runs, one per machine |

## Adding the verification machines

The budgets are defined on a Raspberry Pi 4 and a Ryzen 7 2700X (`requirements/performance.md`
§1). Build on the machine itself (or cross-compile the host and `guest-process`), run
`plugin-spike all --json results/<machine>.json > results/<machine>.md`, and add the numbers
to `design/plugins.md`.

## The lyrics plugin

Beyond the measurements, this workspace holds a real plugin and the runner that runs it:

| Path | |
|---|---|
| `wit/lyrics/plugin.wit` | The plugin contract: scoped library reads, `save-lyrics`, `http.get` to approved destinations, and the host's `granted`/`log` |
| `lrclib-lyrics/` | The plugin: finds tracks without lyrics, asks lrclib.net, and saves `.lrc` files beside them. `manifest.json` is what the Plugins page shows |
| `runner/` | `plugin-run <id> --data <dir>`: reads what the Plugins page installed and approved, and runs the plugin with only those permissions |

To use it end to end:

```sh
./build.sh                                          # also packs target/lrclib-lyrics.wasm
JEWELCASE_DATA_DIR=./data cargo run -p jewelcase-server   # from the repo root, so the scanner picks up new files
cd web && pnpm dev
```

Then, on the Plugins page:
1. Install `spikes/plugins/target/lrclib-lyrics.wasm`, approve its permissions, and enable it.
2. Press **Run now**.
3. Play a song and open **Lyrics** in the player.

Without write access, the plugin reports what it found but saves nothing.
