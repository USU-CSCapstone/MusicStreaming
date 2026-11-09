# General Design

## Overview
The core stack and system architecture. Subsystem designs build on this file and choose their own feature-specific libraries. Where it conflicts with [`requirements/`](../requirements/), the requirements win.

---

## 1. Stack

| Layer | Choice |
|---|---|
| Server | Rust (Tokio, Axum) |
| Shared core | Rust — native on the server and Android, WebAssembly in the web client |
| Web client | Svelte 5 + TypeScript on SvelteKit in SPA mode — static output, no Node server |
| Android | Native Kotlin app, built after the web client |
| Database | SQLite, on the server and, for the device's copy of the catalog, in the browser |
| Distribution | One container image, `linux/amd64` and `linux/arm64` |

- **Rust.** Offline parity ([`requirements/general.md` §3.5](../requirements/general.md#35-offline-parity)) requires search, sort, and shuffle to be a single implementation on every platform, and Rust compiles one to the server, the browser, and Android. It also fits low-power hardware's memory limits and keeps tail latency free of GC pauses ([`requirements/performance.md` §1](../requirements/performance.md#1-target-hardware), [§3](../requirements/performance.md#3-interactions)). Cost: compile times and iteration speed.
- **Svelte 5.** Fine-grained updates suit the one-frame input budget.
- **SvelteKit, SPA mode only.** It provides routing, nested layouts, per-route code splitting, and navigation that waits for a page's data, so views arrive complete ([`requirements/conventions.md` §3](../requirements/conventions.md#3-rendering)). These are maintained upstream and familiar to contributors. The static adapter emits plain files the Rust server serves. There is no Node server in production, which would break the single container and cost Pi memory, and no server routes, which would bypass the public API ([§6](#6-public-api)). Cost: SvelteKit fixes the base path at build time ([§7.1](#71-web)).
- **Native Android.** Background audio, media-session and car integration, and durable downloads ([`requirements/playback.md` §7](../requirements/playback.md#7-system-integration), [`requirements/offline.md` §11](../requirements/offline.md#11-platform-parity)) are first-class on the platform rather than bridged. iOS remains the installed web app ([`requirements/general.md` §4](../requirements/general.md#4-target-platforms)).
- **SQLite.** A database server would break single-directory backups and tag-only upgrades ([`requirements/deployment.md` §4](../requirements/deployment.md#4-backup--restore)–[§5](../requirements/deployment.md#5-upgrades)). The workload is read-heavy and fits comfortably. A device keeps its catalog in SQLite too, so the queries that browse it are the server's own ([§3.1](#31-the-catalog-queries), [ADR 0008](../adr/0008-sqlite-device-catalog.md)).

---

## 2. System Shape

- **One server process in one container.** No sidecars, broker, or external services.
- **Music is mounted writable by default**, so plugins granted write access can save into it ([`requirements/plugins.md` §3](../requirements/plugins.md#3-writing-to-the-library)). The core only reads it ([`requirements/general.md` §3.1](../requirements/general.md#31-the-music-library-is-read-only-to-core)), so a read-only mount works just as well.
- **The data directory splits precious from rebuildable:**
  ```
  data/
  ├── state/   # database, custom artwork, plugin data — back this up
  └── cache/   # transcodes, resized images, indexes — rebuilt on demand
  ```
  The server creates `state/` and `cache/` readable by its own user only, and refuses to start when the data directory and a library's root are the same directory or one holds the other.
- **A stop does first what a kill would lose.** `docker stop` sends SIGTERM and kills the server 10 s later. On SIGTERM or SIGINT the server stops taking connections. As it starts waiting, at most 3 s, for the requests in progress, it closes each realtime connection with a Close frame and saves where every account was playing. An API request takes milliseconds, so what is left at the deadline is a stream or an upload, which its client resumes. Last it asks every library's scanner and background job to stop after the file in hand or between reads of a decode ([`scanning.md` §11](scanning.md#11-ffmpeg)), before waiting for any, so the libraries stop together. A second SIGTERM or SIGINT during the stop ends the process at once, with exit code 130.
- **Work is prioritized:** playing streams, then interactive requests, then background work (scanning, analysis, images, plugin jobs). New streams are refused when the host cannot take them without harming existing ones ([`requirements/performance.md` §7](../requirements/performance.md#7-under-saturation)).

---

## 3. Shared Core

One crate holds every rule that must behave identically on the server and a device ([`requirements/search.md` §2.1](../requirements/search.md#21-one-search-two-places-to-run-it)):

- Domain model and identifiers
- Normalization and sort keys ([`requirements/conventions.md` §4](../requirements/conventions.md#4-sorting--browsing))
- What search covers, its matching and ranking, and the search index's format (built from the catalog's rows, in memory)
- Queue operations and shuffle ([`requirements/queue.md`](../requirements/queue.md))
- Similarity scoring for offline endless play ([`requirements/recommendations.md` §10](../requirements/recommendations.md#10-offline))

Rules:

- **Pure.** No I/O, clock, or platform APIs; callers pass in what it needs.
- **Deterministic.** Shuffle is seeded and the seed is stored with the queue. Every ordering ends in a tie-break on a stable ID. A sorted list of tracks compares sort keys, then ends on the track ID, then on the track's position in the unsorted list, with unknown values last, so the same sort of the same list gives the same order on the server and every device. The comparator is the core's (`sort_rows`): the web sorts the lists it holds through `core-wasm`, and the server sorts a playlist's rows with it, so a recipe resolves to the order a page shows. Nothing that decides an order may differ between native and WebAssembly builds.
- **Sort keys are computed by the core at scan time** and synced. No database collation or client locale ever decides an order.
- **Clients never reimplement a core rule.** Any client code that orders, matches, or selects tracks calls the core.

### 3.1 The catalog queries

The core stays small enough for the page thread, so it holds no SQL. The SQL that reads the catalog lives in `crates/catalog`, with the device's table definitions and a version:

- **One set of queries, two databases.** Listings with their filters, sorts and keyset paging, single gets, the search response and the rows its index is built from, playlist reads and queue recipes all take a database connection and a library, and never check access, which stays the server's gate ([§6](#6-public-api)). The server's handlers call them. On a device the catalog worker runs the same code, compiled to WebAssembly.
- **The device schema is a subset of the server's.** The tables, columns and types are the server's, minus what never leaves it: file paths, roots, file stamps, scan and analysis state, plugin data and other users' data ([`requirements/general.md` §3.7](../requirements/general.md#37-payloads-carry-only-what-the-client-needs)). Counts the server keeps by trigger arrive as plain columns. A test checks that every device column exists in the server's schema with the same type.
- **Parity is tested.** A test builds a server database, syncs a device database from its feeds, and checks that every shared query returns identical JSON from both. The worker's build checks the same results.
- **A device holds missing tracks too,** as the server does: a listing shows them and a recipe keeps them in place, so what a start queues is what its page shows ([`requirements/conventions.md` §6](../requirements/conventions.md#6-availability)). The sort indexes that serve a listing serve a recipe's order, and a counted tag total starts from its smallest tag group.
- **A device copy is a cache.** It is deleted and downloaded again when its schema version changes, a query finds it corrupt, or the browser evicts it ([§7.1](#71-web)).

---

## 4. Sync and Mutations

- **Change feeds.** Each library has an ordered feed of content changes, and each account has one for its personal data, so far playlists and their items ([`database.md` §5](database.md#5-change-feeds)). Clients sync from a cursor, coalesced to net effect, and an expired cursor falls back to a full resync. A cursor is the feed's own position, counted by that library's or account's changes alone so it tells nothing of others, with the epoch of the server's run that gave it, so a server restored from a backup refuses a cursor from beyond the backup. A device with no copy of a library starts from a snapshot file the server builds, served gzipped as it is stored so a download cut short resumes, then follows the feed from the snapshot's position. Plugins follow the same feeds with durable cursors ([`requirements/plugins.md` §8](../requirements/plugins.md#8-events)).
- **Domain outboxes.** A hook that tells a plugin about one user's activity reads an outbox that activity's own domain keeps, written in the same transaction as the activity: `play_events` for `played` and `playing`, and `search_events` for `searched` ([`hooks.md` §1](hooks.md#1-built-hooks)). Each holds only what its hooks deliver, and new personal activity gets an outbox of its own rather than joining a shared log, so the account feed takes a new kind only when a device needs to sync it.
- **Commands.** Every user action applies locally, then goes to the server as an idempotent command with a client-generated ID, queued durably on the device. Offline is the same path with a longer wait ([`requirements/general.md` §3.4](../requirements/general.md#34-optimistic-by-default)). Until the durable queue is built, a command is retried only while the page is open, so an edit made offline is lost if the app closes before it reconnects.
- **Plays** carry the time they happened, not the time they synced.
- **Playlist edits** carry the version they were made against. A device's queued edits chain versions, so they never refuse each other ([`requirements/playlists.md` §5](../requirements/playlists.md#5-concurrent-edits)).
- **Playback control** is last-command-wins, ordered at the server.

---

## 5. Realtime

- **One WebSocket per client**, carrying feed notifications, playback state, and Shift commands.
- **The server owns playback state** per account and decides which device is playing ([`requirements/realtime.md` §2](../requirements/realtime.md#2-one-player-at-a-time)).
- **Heartbeats** detect lost connections. Reconnection resumes from cursors and receives current state, not a replay.
- **Feed notifications are hints.** The server tells each connected account that a library it reaches (`libraryChanged`), or the account's own data (`accountChanged`), has moved, with that feed's own new position. A library's hints are throttled to one a second, the last of a burst always sent, so an import of ten thousand tracks is a handful of messages. A device pulls its feed on every hint and on every `hello`, so live updates and catching up after a disconnect are one path, and a lost hint costs nothing.

---

## 6. Public API

- **First-party clients use only the public API.** Nothing is private to them.
- **JSON over HTTP**, with a compact encoding allowed for bulk sync — still documented.
- **The spec is written first.** `api/openapi.yaml` is the contract; the server is tested against it, and CI lints it and diffs it to catch breaking changes.
- **Responses are purpose-built types**, never serialized database rows ([`requirements/general.md` §3.7](../requirements/general.md#37-payloads-carry-only-what-the-client-needs)).
- **Every query on library content takes an authenticated scope** — the account and the libraries it can reach — as a required argument. Client-supplied IDs select within the scope and never widen it. Plugins use the same scoped layer ([`requirements/general.md` §3.6](../requirements/general.md#36-libraries-are-the-isolation-boundary), [`requirements/users.md` §10](../requirements/users.md#10-access-semantics)). In the server, the auth gate checks that the account reaches the library a path names, in one SQL test shared by every query that needs it, and only then hands the handler a `Scope`: the account and that library. A handler takes its library from the `Scope`, never from the path.

---

## 7. Clients

### 7.1 Web
- **Static files served by the server.** Unknown paths fall back to the SPA entry page.
- **The base path is rewritten at startup.** The client is built with a placeholder base path. At startup the server writes a copy of the build into `cache/` with the placeholder replaced by the configured prefix, so one image works under any path ([`requirements/deployment.md` §6](../requirements/deployment.md#6-remote-access)). CI runs the client under a non-root prefix.
- **Pages read the device's copy once a library is on it.** A page's `load` awaits its first query, and live changes reach the page when the worker applies them, which reloads it in place. Where a page's answer comes from depends on the library's state: the server until the library has been synced once (offline, the page says the library is not on this device yet, never part of it), and the device alone once it is there, even while it is catching up, since the catch-up replaces what is shown in place. Reads made for a write, such as a playlist's version, always go to the server.
- **Threads:** the UI thread handles rendering, input, and audio; two workers hold the rest: the queue worker holds the core (WebAssembly) and the queue, and the catalog worker holds the catalog, the search index over it and sync; a service worker holds the app shell and cached imagery and, once built, downloaded audio. A command queue that survives a restart is not built. One tab at a time owns the catalog worker, since the browser's file handle for the database is exclusive; the others, and the queue worker, ask it ([`web.md`](web.md#catalog-worker)). A tab where the browser has no private file system reads from the server instead. One exception: a page that sorts a list it holds (an album, a playlist, a sorted search) loads the core on the UI thread too, so the sort stays synchronous as the page renders it ([§3](#3-shared-core)); only those pages import it, and they and the queue worker share one download of the WebAssembly file.
- **The UI thread never does work proportional to library size.** Lists are virtualized.
- **Search runs on the server when reachable**, on the device otherwise or when the server is slow — the same core either way.

### 7.2 Android
- **Native Kotlin**, calling the shared core through native bindings.
- **Same public API, feeds, and command model** as the web client.

---

## 8. Plugins

**Plugins are WebAssembly components hosted in-process by Wasmtime**, settled by a spike against an out-of-process baseline ([`plugins.md`](plugins.md)). Constraints ([`requirements/plugins.md` §2](../requirements/plugins.md#2-a-fast-system-not-restricted-plugins), [§11](../requirements/plugins.md#11-isolation--boundaries)): near-zero call overhead, warm instances, isolation from crashes and hangs, bounded calls, and batched data access through the scoped layer ([§6](#6-public-api)). In-process calls cost under a microsecond against 14 µs through a pipe. Deadlines, memory limits, and pauses hold for every guest. The library scope is bound into each instance rather than passed by the plugin.

Plugin UI surfaces must render natively on both web and Android ([`requirements/plugins.md` §9](../requirements/plugins.md#9-extending-the-interface)), which points to a declarative description rather than shipped web code.

---

## 9. Repository

```
jewelcase/
├── crates/
│   ├── catalog/     # the device catalog's schema, the sync rows and snapshot, and the SQL the server and the device share (§3.1)
│   ├── catalog-wasm/ # WebAssembly bindings for the catalog: SQLite on OPFS in the catalog worker (§7.1)
│   ├── core/        # shared core (§3)
│   ├── core-wasm/   # WebAssembly bindings for the web client: sort, list hash and the queue
│   ├── ffmpeg/      # ffmpeg subprocess wrapper — probe, decode, transcode (scanning.md §11)
│   ├── plugins/     # plugin host — Wasmtime, the WIT contract, manifests (plugins.md)
│   ├── scanner/     # ingestion pipeline (scanning.md)
│   └── server/
├── plugins/         # example plugins, each its own workspace, and the author guide
├── tools/           # plugin-pack, which embeds a manifest in a plugin
├── web/             # SvelteKit client (SPA mode)
├── android/         # native app (later)
├── api/             # OpenAPI spec — the API contract
├── bench/           # full-scale corpus with sidecars and corrupt files (not yet; the generated library is in crates/server/src/db/testing/scale.rs)
├── Dockerfile, compose.yaml  # development containers
├── adr/
├── design/
└── requirements/
```

---

## 10. Verification

- **Parity tests:** every build of the core produces byte-identical results for the same inputs.
- **Isolation tests:** cross-library and cross-account access attempted through every endpoint.
- **API compatibility check** in CI ([§6](#6-public-api)).
- **Benchmarks** against a generated large library, on low-power hardware as well as typical machines, reporting percentiles. Regressions are defects ([`requirements/performance.md` §8](../requirements/performance.md#8-measurement)).

---

## 11. Open Decisions

1. **Audio delivery** — gapless background playback in the browser, above all in an installed iOS web app, and the stream format that serves it.
2. ~~**On-device catalog and index format**~~ Settled: SQLite with the catalog's SQL shared ([§3.1](#31-the-catalog-queries), [ADR 0008](../adr/0008-sqlite-device-catalog.md)). A 500,000-track copy is about 360 MB, within [`requirements/performance.md` §6](../requirements/performance.md#6-client-footprint).
3. **Track identity** that survives retags and moves ([`requirements/scanning.md` §6](../requirements/scanning.md#6-track-identity)) without slowing cold scans ([`requirements/performance.md` §5](../requirements/performance.md#5-scanning--analysis)).
4. **Plugin UI description** ([§8](#8-plugins)). The execution model is settled ([`plugins.md`](plugins.md)); its remaining questions are listed there.
5. **Project license.**
