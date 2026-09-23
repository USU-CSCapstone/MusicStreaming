# General Design

## Overview
The core stack and system architecture. Subsystem designs build on this file and choose their own feature-specific libraries. Where it conflicts with `requirements/`, the requirements win.

---

## 1. Stack

| Layer | Choice |
|---|---|
| Server | Rust (Tokio, Axum) |
| Shared core | Rust — native on the server and Android, WebAssembly in the web client |
| Web client | Svelte 5 + TypeScript on SvelteKit in SPA mode — static output, no Node server |
| Android | Native Kotlin app, built after the web client |
| Database | SQLite |
| Distribution | One container image, `linux/amd64` and `linux/arm64` |

- **Rust.** Offline parity (`requirements/general.md` §3.5) requires search, sort, and shuffle to be a single implementation on every platform, and Rust compiles one to the server, the browser, and Android. It also fits the Pi's memory limits and p95/p99 budgets without GC pauses (`requirements/performance.md` §1, §3). Cost: compile times and iteration speed.
- **Svelte 5.** Fine-grained updates suit the one-frame input budget.
- **SvelteKit, SPA mode only.** It provides routing, nested layouts, per-route code splitting, and navigation that waits for a page's data, so views arrive complete (`requirements/conventions.md` §3). These are maintained upstream and familiar to contributors. The static adapter emits plain files the Rust server serves. There is no Node server in production, which would break the single container and cost Pi memory, and no server routes, which would bypass the public API (§6). Cost: SvelteKit fixes the base path at build time (§7.1).
- **Native Android.** Background audio, media-session and car integration, and durable downloads (`requirements/playback.md` §7, `requirements/offline.md` §11) are first-class on the platform rather than bridged. iOS remains the installed web app (`requirements/general.md` §4).
- **SQLite.** A database server would break single-directory backups and tag-only upgrades (`requirements/deployment.md` §4–§5). The workload is read-heavy and fits comfortably.

---

## 2. System Shape

- **One server process in one container.** No sidecars, broker, or external services.
- **Music is mounted read-only**, so `requirements/general.md` §3.1 is enforced by the filesystem. A writable mount is the admin's choice, for plugins.
- **The data directory splits precious from rebuildable:**
  ```
  data/
  ├── state/   # database, custom artwork, plugin data — back this up
  └── cache/   # transcodes, resized images, indexes — rebuilt on demand
  ```
- **Work is prioritized:** playing streams, then interactive requests, then background work (scanning, analysis, images, plugin jobs). New streams are refused when the host cannot take them without harming existing ones (`requirements/performance.md` §7).

---

## 3. Shared Core

One crate holds every rule that must behave identically on the server and a device (`requirements/search.md` §2.1):

- Domain model and identifiers
- Normalization and sort keys (`requirements/conventions.md` §4)
- Search matching, ranking, and the on-device index format
- Queue operations and shuffle (`requirements/queue.md`)
- Similarity scoring for offline endless play (`requirements/recommendations.md` §10)

Rules:

- **Pure.** No I/O, clock, or platform APIs; callers pass in what it needs.
- **Deterministic.** Shuffle is seeded and the seed is stored with the queue. Every ordering ends in a tie-break on a stable ID. Nothing that decides an order may differ between native and WebAssembly builds.
- **Sort keys are computed by the core at scan time** and synced. No database collation or client locale ever decides an order.
- **Clients never reimplement a core rule.** Any client code that orders, matches, or selects tracks calls the core.

---

## 4. Sync and Mutations

- **Change feeds.** Each library has an ordered feed of content changes; each account has one for its personal data. Clients sync from a cursor, coalesced to net effect, and an expired cursor falls back to a full resync. Plugins follow the same feeds with durable cursors (`requirements/plugins.md` §8).
- **Commands.** Every user action applies locally, then goes to the server as an idempotent command with a client-generated ID, queued durably on the device. Offline is the same path with a longer wait (`requirements/general.md` §3.4).
- **Plays** carry the time they happened, not the time they synced.
- **Playlist edits** carry the version they were made against. A device's queued edits chain versions, so they never refuse each other (`requirements/playlists.md` §5).
- **Playback control** is last-command-wins, ordered at the server.

---

## 5. Realtime

- **One WebSocket per client**, carrying feed notifications, playback state, and Shift commands.
- **The server owns playback state** per account and decides which device is playing (`requirements/realtime.md` §2).
- **Heartbeats** detect lost connections. Reconnection resumes from cursors and receives current state, not a replay.

---

## 6. Public API

- **First-party clients use only the public API.** Nothing is private to them.
- **JSON over HTTP**, with a compact encoding allowed for bulk sync — still documented.
- **The spec is written first.** `api/openapi.yaml` is the contract; the server is tested against it, and CI lints it and diffs it to catch breaking changes.
- **Responses are purpose-built types**, never serialized database rows (`requirements/general.md` §3.7).
- **Every query on library content takes an authenticated scope** — the account and the libraries it can reach — as a required argument. Client-supplied IDs select within the scope and never widen it. Plugins use the same scoped layer (`requirements/general.md` §3.6, `requirements/users.md` §10).

---

## 7. Clients

### 7.1 Web
- **Static files served by the server.** Unknown paths fall back to the SPA entry page.
- **The base path is rewritten at startup.** The client is built with a placeholder base path. At startup the server writes a copy of the build into `cache/` with the placeholder replaced by the configured prefix, so one image works under any path (`requirements/deployment.md` §6). CI runs the client under a non-root prefix.
- **Pages load from the worker, not the network.** A page's `load` awaits its first query to the worker, and live changes reach the page through Svelte stores fed by the change feeds.
- **Three threads:** the UI thread handles rendering, input, and audio; a worker holds the core (WebAssembly), catalog, search, sync, and command queue; a service worker holds the app shell and downloaded audio and imagery.
- **The UI thread never does work proportional to library size.** Lists are virtualized.
- **Search runs on the server when reachable**, on the device otherwise — the same core either way.

### 7.2 Android
- **Native Kotlin**, calling the shared core through native bindings.
- **Same public API, feeds, and command model** as the web client.

---

## 8. Plugins

The execution model will be settled by a spike. Constraints (`requirements/plugins.md` §2, §11): near-zero call overhead, warm instances, isolation from crashes and hangs, bounded calls, and batched data access through the scoped layer (§6). The leading candidate is WebAssembly components hosted in-process.

Plugin UI surfaces must render natively on both web and Android (`requirements/plugins.md` §9), which points to a declarative description rather than shipped web code.

---

## 9. Repository

```
jewelcase/
├── crates/
│   ├── core/        # shared core (§3)
│   ├── core-wasm/   # WebAssembly bindings for the web client
│   └── server/
├── web/             # SvelteKit client (SPA mode)
├── android/         # native app (later)
├── api/             # OpenAPI spec — the API contract
├── bench/           # library generator and benchmark harness
├── docker/          # image and reference compose file
├── design/
└── requirements/
```

---

## 10. Verification

- **Parity tests:** every build of the core produces byte-identical results for the same inputs.
- **Isolation tests:** cross-library and cross-account access attempted through every endpoint.
- **API compatibility check** in CI (§6).
- **Benchmarks** against a generated full-scale library on both verification machines, at p95 and p99. Regressions block releases (`requirements/performance.md` §8).

---

## 11. Open Decisions

1. **Audio delivery** — gapless background playback in the browser, above all in an installed iOS web app, and the stream format that serves it.
2. **On-device catalog and index format**, within `requirements/performance.md` §6.
3. **Track identity** that survives retags and moves (`requirements/scanning.md` §6) within the cold-scan budget (`requirements/performance.md` §5).
4. **Plugin execution model and UI description** (§8).
5. **Project license.**
