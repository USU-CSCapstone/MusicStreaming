# Plugins Design

## Overview
How plugin code runs. This file records the execution-model spike that [`general.md` §8](general.md#8-plugins) called for, and the decision it supports. It builds on [`requirements/plugins.md`](../requirements/plugins.md); where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **a plugin costs its own work and nothing else** ([`requirements/plugins.md` §2](../requirements/plugins.md#2-a-fast-system-not-restricted-plugins)).

The spike lives in [`spikes/plugins/`](../spikes/plugins/). Its raw results are in [`results/apple-m5.md`](../spikes/plugins/results/apple-m5.md) and [`.json`](../spikes/plugins/results/apple-m5.json).

---

## 1. Decision

**Plugins are WebAssembly components hosted in-process by Wasmtime.** The spike measured this against an out-of-process baseline, where each plugin is a native child process on a pipe. Both used the same operations and the same scoped data, with Rust and JavaScript guests.

What decided it:

| Constraint | In-process Wasm (Rust guest) | Out-of-process | |
|---|---|---|---|
| Call overhead, p99 | **0.75 µs** | 14 µs | about 18× cheaper |
| Reading data one item per call | **284 ns/track** | 9.9 µs/track | about 35× cheaper; batching stops mattering |
| Reading data in pages of 1,000 | **153 ns/track** | 327 ns/track | half the cost |
| A hung call, 50 ms budget | stopped at 50 ms, replaced in 0.05 ms | killed at 53 ms, respawned in 1.1 ms | |
| Runaway allocation | **refused at the limit**, freed on drop | 1 GiB allocated unchecked | processes need cgroups, which the container may not have |
| Pausing for listeners | parks within a tick, 0.3 ms | only by `SIGSTOP` from outside | |
| Idle footprint | **41 KB** per plugin | 1.7 MB per plugin | |

- **The out-of-process model loses on everything the requirements weight most.** It adds a cost to every call, it makes per-item access ruinous, and it cannot bound memory portably. [`requirements/plugins.md` §2.1](../requirements/plugins.md#21-the-system-adds-nothing) rules each of these out: "no serialization tax", and "never one entity at a time". Its one advantage is below (§4).
- **Isolation holds with no sandbox promises.** A crash is a trap. A hang is stopped by a deadline, and memory by a limiter. A fresh instance is ready in microseconds. No failure reached the host or another plugin ([§11](../requirements/plugins.md#11-isolation--boundaries)), and every containment check passed for all three Wasm guests.

---

## 2. Execution Model

- **One engine per server. Components compile once per plugin version and are cached.** Compiling costs 9 ms for Rust and 0.85–1 s for JS. Loading a precompiled artifact costs 0.1–4 ms. Install and update pay the compile, and nothing else ever does.
- **One warm instance per plugin per library.** The library is bound into the instance's store, so no host function takes a library argument, and a plugin cannot name one it was not enabled for ([`requirements/plugins.md` §11](../requirements/plugins.md#11-isolation--boundaries), [`general.md` §6](general.md#6-public-api)). The spike checked this with two instances of the same plugin: each saw only its own library.
- **Any error discards the instance.** A trapped component cannot be re-entered, so it is replaced from its pre-linked form. That takes 7–11 µs for Rust and about 0.4 ms for JS, which is what makes "a plugin that keeps failing" survivable. State meant to outlive an instance goes through the host's `state` interface, not guest memory.
- **Epoch interruption at a 250 µs tick drives three things:**
  - *deadlines*: every call has one, and it is enforced to within a tick;
  - *yields*: a long call gives up its thread every tick;
  - *pauses*: the Governor parks background plugin work at the next tick until listeners are served ([`requirements/plugins.md` §2.2](../requirements/plugins.md#22-results-compose-they-do-not-block), [`scanning.md` §10](scanning.md#10-priority-and-pacing)).
- **The tick is a measured trade.** With plugins spinning on every worker thread, the others waited 3.0 ms at p99 with a 1 ms tick. At 250 µs they waited 1.5 ms, and 100 µs bought nothing more. Call overhead was unaffected at every setting.
- **Plugin calls run on their own executor, never on the threads serving requests.** Within one executor, a plugin can only delay another by up to about a tick. The core cannot afford even that, so core results never wait behind plugin code, and plugin sections fill in on their own ([§2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)).
- **A resource limiter per store** bounds each instance's memory. The spike used 128 MiB.
- **The pooling allocator** cuts Rust instantiation from 11 µs to 7 µs, and matters little for JS.
- **WASI with nothing granted.** No preopens, sockets, or inherited stdio. What a plugin can reach is exactly the host interfaces it is given.

---

## 3. Data Access

**Plugins read library data through scoped host interfaces, in pages, straight from the host's memory.** Nothing goes through the public API or serialization ([`requirements/plugins.md` §2.1](../requirements/plugins.md#21-the-system-adds-nothing)).

- **A 500,000-track pass costs 77 ms from a Rust plugin.** That is 3.2× the host's own paged scan (24 ms). It missed the proposed 2× bar.
- **Where the extra cost comes from.** The canonical ABI lowers a `list<record>` of strings by calling into the guest to allocate each string, and the host copies each page once before that.
- **Untested hypothesis:** a column layout for bulk reads (one buffer of titles plus offsets) would remove the per-string round trips. That belongs in the design of the real data interface ([§6](#6-open-questions)).
- **Batching barely matters in-process.** One track per call costs 284 ns per track, against 153 ns in pages of 1,000. Out of process the same pattern costs 9.9 µs per track. So the system does not have to force batching on authors to stay fast.

---

## 4. Guest Languages

**Compiled languages run at near-native cost. JavaScript works, and costs roughly an order of magnitude more on every axis.** Numbers are for JS built with componentize-js, interpreted, with the AOT (weval) figure after the slash.

| | Rust | JavaScript |
|---|---|---|
| Component size | 72 KB | 12.0 / 15.2 MB |
| Instantiate | 7–11 µs | about 0.4 ms |
| Idle memory per plugin | 41 KB | 8.5 / 9.1 MB |
| Call, p50 | 0.6 µs | 3.7 µs; 10 µs with a 64 B string |
| Reading data | 153 ns/track | 3.3 / 2.7 µs/track |

- **JavaScript is fine for what small plugins mostly do.** Reacting to events, enriching metadata in the background, and scheduled jobs all qualify. "A plugin can be trivial" ([`requirements/plugins.md` §1](../requirements/plugins.md#1-an-open-surface)) holds.
- **It is not fine on a per-keystroke path or for a whole-library pass inside an interactive budget.** 500,000 tracks take 1.3–1.7 s. Plugin cost is attributable to the plugin ([§2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)), and the author documentation should give these numbers.
- **The out-of-process baseline had one edge over JS: memory.** A native process idled at 1.7 MB against JS's 8.5 MB. Twenty JS plugins come to about 170 MB, which the Pi's 4 GB floor absorbs ([`requirements/performance.md` §1](../requirements/performance.md#1-verification-hardware)), but it should be watched.
- **AOT compilation buys 20% on compute and costs 3 MB and slightly more memory.** Whether to use it is the plugin author's choice, not the system's.

---

## 5. Method

- **Machine:** Apple M5, 10 cores, macOS. The host ran tokio with 4 worker threads to imitate the Pi 4.
- **Versions:** Wasmtime 49 with its WASI p2 implementation. Guests: Rust through wit-bindgen for `wasm32-wasip2`; JS through componentize-js 0.23; the process baseline as length-prefixed JSON over stdin/stdout.
- **Catalog:** synthetic, 500,000 plus 20,000 tracks, held in memory so SQLite's paging does not blur the boundary cost.
- **Assertions:** containment, scoping, and state are asserted, and `cargo test` covers the fast versions. Performance bars were proposals and are reported rather than asserted. The spike's interface is [`spikes/plugins/wit/plugin.wit`](../spikes/plugins/wit/plugin.wit).
- **Not yet run on the verification machines.** The budgets are defined on the Pi 4 and the Ryzen 7 2700X ([`requirements/performance.md` §1](../requirements/performance.md#1-verification-hardware)). The harness is one binary that cross-compiles, and those runs are the first open item below.

---

## 6. Open Questions

1. **Pi 4 and Ryzen runs.** The decision rests on ratios, which should hold, but the budgets are absolute.
2. **Plugin UI description.** This is the other half of [`general.md` §11](general.md#11-open-decisions) #4: the declarative format both clients render ([`requirements/plugins.md` §9](../requirements/plugins.md#9-extending-the-interface)).
3. **The real host interface.** It needs a bulk read encoding (§3), events with replay ([`requirements/plugins.md` §8](../requirements/plugins.md#8-events)), settings and credentials ([§6](../requirements/plugins.md#6-configuration--credentials)), and a writable preopen limited to the library's roots for plugins that write files ([§3](../requirements/plugins.md#3-writing-to-the-library)).
4. **Network access and async host calls.** A plugin waiting on a remote service must not hold its instance's thread. Wasmtime supports async host functions and `wasi:http`, but neither was measured.
5. **Concurrent calls to one plugin.** One instance serializes its calls. Whether a busy plugin gets a small pool of instances, and how its state is shared across them, is untested.
