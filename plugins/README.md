# Writing a plugin

A Jewelcase plugin is one WebAssembly component with its manifest packed in. An admin installs
it on the Plugins page, approves what it asks for, and enables it per library. This guide is
what an author needs to know. The design behind it is
[`design/plugins.md`](../design/plugins.md), and what a plugin can call and be called with is in
[`imports.md`](../design/imports.md), [`hooks.md`](../design/hooks.md), and
[`exports.md`](../design/exports.md).

The two plugins here are the examples to copy:
- [`lrclib-lyrics`](lrclib-lyrics/) works through the whole library in parts, writes files, and
  retries outages.
- [`listenbrainz`](listenbrainz/) acts for each user who connects it, with their own token.

## The contract

The contract is `jewelcase:plugin@0.1.0`, one WIT file:
[`crates/plugins/wit/plugin.wit`](../crates/plugins/wit/plugin.wit). It names the imports a
plugin can call, the events it is sent, and its one export, `handle(event)`, which returns a
one-line summary or a reason it failed.

The examples' Rust bindings read it from there:

```rust
wit_bindgen::generate!({ path: "../../crates/plugins/wit", world: "plugin" });
```

To copy a plugin out of this repository, copy `crates/plugins/wit` beside it too and edit that
path. The manifest's `apiVersion`, `"0.1"`, names the contract version you copied; copy it
again when the contract changes.

## The manifest

`manifest.json` names the plugin and asks for its permissions and hooks, each with the reason
the admin sees. Its fields are in the
[plugin-pack README](../tools/plugin-pack/README.md#the-manifest), and the rules in
[`design/plugins.md` §6](../design/plugins.md#6-packaging). It is strict: a key it does not
know is refused at any level, `destinations` are host names (not URLs, ports, or patterns) or
`["*"]`, and a setting's `default` must be one of its choices. The server is the only validator,
so mistakes show at install, with what to fix.

## Building and packing

```sh
rustup target add wasm32-wasip2   # once
./build.sh                        # target/<id>.wasm, ready to install
```

`build.sh` builds for `wasm32-wasip2` in release, then packs `manifest.json` into the component
with [`tools/plugin-pack`](../tools/plugin-pack/), which needs Node. It runs the packer at
`../../tools/plugin-pack/pack.mjs`; outside this repository, point that line at
`tools/plugin-pack/pack.mjs` in a Jewelcase checkout. Copy its
[`Cargo.toml`](lrclib-lyrics/Cargo.toml) too: `crate-type = ["cdylib", "rlib"]` so the logic
also builds natively for tests, `wit-bindgen` only for `wasm32`, and an empty `[workspace]`.

## Budgets

Each run is one call to `handle` with these limits. Running out of compute or wall clock ends
the run as a failure. The others shorten or refuse the call: a page or read gives at most its
limit, a value, a write or a request body over its limit is refused, and a reply over it
fails. Log lines past the limit are dropped.

| | Limit | Defined in |
|---|---|---|
| Compute | 60,000 ticks of 1 ms, about 63 s. Waiting costs nothing, but every host call costs a tick however long it takes. | `crates/plugins/src/host.rs` |
| Wall clock | 5 minutes, waiting included | `host.rs` |
| Memory | One linear memory, up to 64 MiB. A refused `memory.grow` returns -1. | `host.rs` |
| Store | 10 tables of up to 100,000 elements, 20 core instances, 10,000 WASI resources | `host.rs` |
| Log | 1,000 `host.log` lines, each cut to 500 characters | `host.rs` |
| `state` | 512 B keys, 1 MiB values, per plugin and library | `host/library.rs` |
| Library | 500 per page or per list asked for by ID | `host/library.rs` |
| Files | 500 entries per `files.entries` call, 8 MiB per read, 32 MiB per write | `files.rs` |
| Network | 15 s for a reply, counted from when the request is sent (connecting gives up after 10 s); 16 MiB bodies (a request body over it is refused, and a reply over it fails); 5 redirects | `host/http.rs` |
| Pacing | A request waits for its turn under the manifest's `rateLimits`, but is refused as `rate-limited` if that is more than 60 s away. A `429` or `503` pauses its scope for its `Retry-After`, or a random wait up to 1 s that doubles with each in a row, up to an hour either way. | `network/limits.rs` |
| Hooks | Up to 100 changes, plays, or searches per event | `crates/server/src/plugins/hooks.rs` |

How these are enforced, and what they do not bound, is in
[`design/plugins.md` §7](../design/plugins.md#7-running-plugins).

**One memory.** A component may have one linear memory and no more. Rust through wit-bindgen
and JavaScript through componentize-js each build one. A component composed from several parts,
as `wac` or `wasm-compose` make, has a memory for each, and install refuses it.

**JavaScript costs more than Rust:** a 12 MB component against 72 KB, 8.5 MB idle against
41 KB, and about 20 times as long to read the library, 1.3 to 1.7 s for 500,000 tracks. That is
fine for events and background work, and not for a whole-library pass someone is waiting on
([`design/plugins.md` §4](../design/plugins.md#4-guest-languages)).

## Long work: a part per run

A run has 5 minutes, so work over the whole library never tries to finish in one. It does a
part, keeps its place in `state`, and carries on next run. lrclib's pass
([`src/pass.rs`](lrclib-lyrics/src/pass.rs)) is the pattern:

- **A cursor.** It reads the library 500 tracks at a time, each page after the last ID of the
  one before. After each lookup it saves that track's ID under `cursor`, so a run cut short
  loses nothing it did. At the end of the library it deletes the cursor and the next run starts
  over.
- **A time stop.** It stops *starting* lookups 120 s into a run, early enough that the last one
  it starts still finishes inside 5 minutes. A lookup is at most two requests, each waiting up to
  60 s for its turn and 15 s for a reply, so 120 + 150 = 270 s. Size yours the same way.
- **A host-call cap.** Each call costs a tick of compute, so it checks at most 10,000 tracks
  without lyrics a run, and reads the clock only after a lookup.
- **Work left from a hook.** A `tracksChanged` batch it has no time for is kept under `next`, up
  to 1,000 tracks, and looked up first by the next run.
- **Something to come back to.** The `schedule` hook (here hourly) brings the next run. The
  summary says there is more to do, since nothing yet tells the host
  ([`imports.md` §3.1](../design/imports.md#progress-and-checkpoints)).

## Errors and retries

Every import that can fail answers one error
([`imports.md` §1](../design/imports.md#1-built-imports)):

| Error | Meaning | What to do |
|---|---|---|
| `not-granted` | Its permission was declined | Adapt up front with `host.granted()`, or skip the work. |
| `refused(why)` | Never done as asked | Count the item as failed and go on: asking again won't help. |
| `failed(why)` | Tried, and it didn't work | From `http.send`, stop and retry later. From `files`, such as a file that already exists, it depends. |
| `rate-limited(ms)` | `http.send` only: not sent, the turn is `ms` away | Stop, and retry later. |
| `timed-out` | `http.send` only: no reply in time | Stop, and retry later. |

**To retry, return `Err` from `handle`** before recording that item as done. A hook's position
moves only when a run succeeds, so the same batch, or the same place in your pass, comes again
after 1 minute, doubling to an hour. **Run now** is not retried. Treat a service's own `429` or
`5xx` answer the same way. lrclib stops on everything except `refused`, and on those answers;
only its `404` counts as having nothing ([`src/failure.rs`](lrclib-lyrics/src/failure.rs)).

**Five failures in a row disable the plugin** in that library, with the reason shown to admins.
A failure the host saw as an **outage** doesn't count toward the five: one after a request was
`rate-limited`, `timed-out`, could not connect, or was answered `429` or a `5xx`. The host sees
that for itself, so you need not report it. Outages back off the same way, and only 72 hours of
them in a row disable the plugin. A success, or any other failure, ends the outage. A hook run
for one user (`played`, `playing`, `searched`) backs off the same way, but never disables the
plugin for anyone else ([`design/plugins.md` §7](../design/plugins.md#7-running-plugins)).

## Testing natively

Keep the logic in plain Rust that doesn't touch the bindings, and only the glue in a
`#[cfg(target_arch = "wasm32")] mod plugin` holding `generate!()`, the `Guest` impl, and
`export!`. Then `cargo test` runs the logic on your machine.

- **Put the imports behind a trait.** lrclib's pass takes a [`Store`](lrclib-lyrics/src/pass.rs)
  of tracks and `state`. In a run that's the imports
  ([`src/plugin/imports.rs`](lrclib-lyrics/src/plugin/imports.rs)); in its tests, a store in
  memory ([`src/pass/tests.rs`](lrclib-lyrics/src/pass/tests.rs)).
- **Pass requests in as a function.** ListenBrainz's `listens::submit` takes the call that posts
  a batch, and its tests ([`src/listens/tests.rs`](listenbrainz/src/listens/tests.rs)) answer
  as a ListenBrainz that refuses or fails.
- **Lint both targets**, since the glue only compiles for wasm:
  `cargo clippy --all-targets` and `cargo clippy --target wasm32-wasip2`.

Then install it on a development server and press **Run now**: the summary and the log lines
show with the run. In this repository, `crates/plugins/tests` also run the packed examples in
the real host, with a library in memory.
