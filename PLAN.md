# Development Plan

How Jewelcase gets from where it is to everything in [`requirements/`](requirements/). This file is about **order, status, and what is still to decide**. What the system must do lives in the requirements, and how it does it lives in [`design/`](design/) and [`adr/`](adr/).

---

## How firm is each part

| Level | What it covers | How it changes |
|---|---|---|
| **Fixed** | The core principles ([`general.md` §3](requirements/general.md#3-core-principles)) and accepted ADRs | Only by a deliberate amendment, or a new ADR that supersedes the old one |
| **Built** | Requirements the code already meets, in the completed stages below | Amend the requirement first, then plan the migration or rework |
| **Open** | Everything in a stage not yet built, including the parts of `api/openapi.yaml` the server doesn't serve | Adjust freely. Settle it at the start of the stage that builds it |

---

## Process

1. **Start a stage by working through its open questions.** Decide each one, and record the decision in the requirements, a design doc, or an ADR. Then update `api/openapi.yaml`, then write code. A question nobody can answer yet gets a spike, or is moved to a later stage on purpose.
2. **Make one small, reviewable change at a time**, one branch and one PR each. Changes stack when they depend on each other.
3. **Close a stage by checking its end goal.** Mark the stage complete, and carry anything deferred into a later stage's open questions.
4. **Test every change at its boundary.** Check library and account isolation on each new endpoint ([`general.md` §3.6](requirements/general.md#36-libraries-are-the-isolation-boundary)). Check new work against [`performance.md`](requirements/performance.md), on low-power hardware once the benchmark harness exists (Stage 13).

The stages are in dependency order. Stages that don't depend on each other can run in parallel, for example 7 with 6, or 12 with 11.

---

## Completed

### Stage 1 — Scanning & catalog ✅

Built: the scanner crate with discovery, every supported format, multi-value tags, sidecars, live watching, scheduled scans, and incremental scans. Missing files are marked rather than removed, and scan problems are logged. A governor yields to listeners. One decode pass yields loudness and waveforms. The SQLite schema and the library change feed are in place.

**Still open from this stage:**
1. **Track identity is keyed on path for now** ([`design/scanning.md` §6](design/scanning.md#6-track-identity)). It moves to Stage 6.
2. Analysis pool sizes and the governor threshold. These are starting values that benchmarks will tune ([`design/scanning.md` §16](design/scanning.md#16-open-decisions)).
3. Whether to keep the denormalized counts, or compute them on read ([`design/database.md` §10](design/database.md#10-open-questions)).
4. Tracks with no disc tag are stored as disc `0` and shown as `null`. Confirm or change this ([`design/database.md` §10](design/database.md#10-open-questions)).
5. The change feed names an album, but not its tracks, when the album's title, artists, or image change, even though track summaries embed the album. Decide this with the offline catalog (Stage 10).

**End goal (reached):** A library on disk becomes a browsable index, kept current as files change, without Jewelcase writing to the library. Most of [`scanning.md`](requirements/scanning.md) is met. Identity (§6), the duplicate report (§9), and the sound description (§1) are not.

### Stage 2 — Browse, search & streaming ✅

Built: an OpenAPI spec written before the server. Endpoints for libraries, tracks, albums, artists, images with ThumbHash placeholders, lyrics, and waveforms, with keyset paging. Fuzzy search in `crates/core` over an in-memory index per library. Direct streaming with range requests. A SvelteKit web client with album, artist, song, and search views, plus a player with a waveform, lyrics, and Media Session.

**Still open from this stage:**
1. The web client doesn't draw ThumbHash placeholders yet. Also decide whether the spec should drop `x-status: draft` on `ImageRef.placeholder`.

**End goal (reached):** A signed-in user can browse, search, and play a library in the web app. [`tracks.md`](requirements/tracks.md), [`albums.md`](requirements/albums.md), and [`artists.md`](requirements/artists.md) are met at the API level, along with server-side matching ([`search.md` §3](requirements/search.md#3-matching)) and direct delivery ([`playback.md` §1](requirements/playback.md#1-delivery)).

### Stage 3 — Accounts & access ✅

Built: first-run owner setup, and login and logout with argon2 and zxcvbn. Sessions are tied to devices. Sign-in limits apply per account and per origin, trusting `X-Forwarded-For` from private-network proxies. The owner, admin, and user roles exist. Library scoping returns `404` for anything out of reach.

**Still open from this stage:**
1. `session_revoked` can't be sent over HTTP, because revocation clears the token hash. Decide this alongside password changes (Stage 7).
2. Nothing reclaims a `deviceId` after `credentials_changed` yet.
3. `login_attempts` is never pruned.

**End goal (reached):** Only the owner can exist, and every request is scoped to what the caller may reach. [`users.md` §2.1, §3, §10](requirements/users.md) and [`libraries.md` §4](requirements/libraries.md#4-isolation) are met.

### Stage 4 — Plugin system ✅

Built: a Wasmtime host (`crates/plugins`) with a versioned WIT contract and manifest validation. Permissions are required or optional, each with a reason, and granted per library or per plugin. Write access is split into adding files and changing them. There are general file, HTTP, state, and library-read imports, plus settings, secrets, and per-user connections. Hooks cover tracks changed, schedules, scans finished, plays, now playing, searches, and albums and artists changed, all with cursors, retries, and auto-disable. There's a shared cache and rate limits. Plays are recorded through `POST /me/plays`. An admin UI ships, along with two reference plugins: lrclib lyrics and ListenBrainz.

**Still open from this stage:**
1. Run the plugin costs on low-power hardware. So far they've only been measured on an M5 ([`design/plugins.md` §8](design/plugins.md#8-open-questions)).
2. Bulk read encoding. A full pass over the library currently costs 3.2× the host's own scan.
3. Revoking a permission mid-run, without a restart.
4. The cost of async host calls, and whether `wasi:http` would be better.
5. Concurrent calls to one plugin: whether to use an instance pool, and how state would be shared across it.
6. Who declares a rate limit, and how long a declaration outlives its plugin (`design/imports.md` §4).
7. Check imports at install: refuse a plugin that imports network access it didn't request.
8. Which hooks need approval. Lifecycle events reach nothing (`design/hooks.md` §5).
9. A plugin whose cursor is older than the change feed's retention horizon.
10. Whether plugins can rely on any order between hooks.

**End goal (reached):** Admins can install a plugin, grant it exactly what it asks for, and have it enrich a library or scrobble plays. [`plugins.md` §1, §3–6, §8, §11](requirements/plugins.md) and [`analytics.md` §10](requirements/analytics.md#10-external-services) are met.

### Stage 5 — Realtime control ✅

Built: device listing, an authenticated WebSocket with heartbeats, live device lists, and player state per account with one playing device. Player state is saved across restarts. Playback stops when the playing device disconnects. Remote commands work over the WebSocket and over HTTP.

**Still open from this stage:**
1. Player state temporarily carries `libraryId` + `trackId`. It moves onto the server queue in Stage 9.
2. Replaying the results of commands that have already finished.

**End goal (reached):** The server knows every device, which one is playing, and where it is in the track, and can route commands to it. The server side of [`realtime.md` §1–2, §4–6, §8](requirements/realtime.md) and [`users.md` §4](requirements/users.md#4-sessions--devices) is met.

---

## Remaining

### Stage 6 — Track identity

- Replace path-keyed identity with identity that follows content and survives moves and retags. Do this before playlists, history, and downloads start to depend on track IDs.

**Open questions:**
1. **What identity reads from a file.** Hashing whole files is ruled out at scale. The candidates are audio-region sampling, tag-independent fingerprints, inode plus size, and MusicBrainz IDs ([`design/general.md` §11](design/general.md#11-open-decisions) #3).
2. How audio replacement is told apart from a new track.
3. How existing path-keyed IDs migrate without breaking play history.

**End goal:** Moving, renaming, or retagging files keeps every track's identity, history, and playlist membership. [`scanning.md` §6–7](requirements/scanning.md#6-track-identity) and [`tracks.md` §1](requirements/tracks.md#1-identity) are met.

### Stage 7 — Administration

- Invites, account suspension and deletion, ownership transfer, password change and reset, signing devices out, and the audit log.
- Library management: create, rename, roots, exclude rules, delete, repointing a moved root, access grants, and a health view.
- Admin views for scan progress and problems, missing tracks and cleanup, and the duplicate report. Account preferences that sync across devices.

**Open questions:**
1. The format for exporting a user's data ([`users.md` §9](requirements/users.md#9-account-lifecycle)).
2. The default invite expiry, and the reset link's lifetime.
3. How `session_revoked` reaches a device (carried over from Stage 3).
4. How duplicates are stored and detected: matching tags, matching content, or the same album under two paths ([`scanning.md` §9](requirements/scanning.md#9-duplicates)).
5. The default interval for scheduled scans.
6. What the admin UI tells users about admin visibility, and where it says it ([`users.md` §8](requirements/users.md#8-admin-capabilities--visibility)).

**End goal:** A household runs one server with several libraries, managed entirely from the admin UI. [`users.md`](requirements/users.md), [`libraries.md` §1–4](requirements/libraries.md), and the admin surfaces of [`scanning.md` §8–10](requirements/scanning.md#8-missing-files) are met.

### Stage 8 — Browsing, search & playlists

- Tag pages and filters, plus [`conventions.md`](requirements/conventions.md) sorting, filtering, and actions everywhere. A library switcher, discography versus appearances, incomplete albums, and virtualized lists.
- Search with a top result, sections, lyric matches, scoping, filters, and recent searches.
- Playlists: CRUD, bulk and non-contiguous edits, version-checked edits, undo, generated artwork, custom artwork, pinning, and export.

**Open questions:**
1. How playlist undo works: a time window, a trash, or both, and how long they last.
2. The format of the complete playlist export ([`playlists.md` §9](requirements/playlists.md#9-export)).
3. How lyric matching and ranking fit into the core search index, without breaking parity with client search.
4. How long recent searches are kept ([`search.md` §6](requirements/search.md#6-recent-searches)).
5. How "near misses" are chosen when a search finds nothing.
6. What a playlist keeps of an external entry: probably a display snapshot the playlist owns (`design/exports.md` §5, Stage 12).

**End goal:** Everything the server holds is reachable and navigable in the web app, and users keep their own playlists. [`conventions.md`](requirements/conventions.md), [`tags.md`](requirements/tags.md), [`search.md` §1, §3–8](requirements/search.md), and [`playlists.md`](requirements/playlists.md) are met, except for generated playlists (§8).

### Stage 9 — Queue, playback & Shift

- A server-owned queue with the Manual Queue, Context Queue, and History. Seeded shuffle in `crates/core`, repeat, and kept sessions. Move player state onto the queue.
- The web client side of realtime: connection, progress, a device picker, and remote control. Then Shift, waking, and per-device volume.
- Transcoding with a cache and seekable streams. Gapless playback, normalization, crossfade, silence skipping, and failure handling.

**Open questions:**
1. **Audio delivery and gapless in the browser**, above all in an installed iOS PWA ([`design/general.md` §11](design/general.md#11-open-decisions) #1). Spike this first, because it decides the stream format.
2. The transcode codec and bitrate for each quality tier, how client capabilities are negotiated, and how the transcode cache is sized and evicted.
3. How seeking works inside a transcoded stream.
4. How a queue over a context of hundreds of thousands of tracks is stored and synced: as a context reference plus a seed, or as a materialized list.
5. How "the user adjusted this queue" is detected, so the queue is kept when a new context starts ([`queue.md` §6](requirements/queue.md#6-sessions)).
6. Silence skipping needs silence detection. It has to come from the existing analysis pass, never a second one.
7. Waveform bin count and shaping ([`design/scanning.md` §16](design/scanning.md#16-open-decisions) #2).
8. Normalization headroom: how quiet tracks are raised without clipping.
9. Whether one browser tab or several can play per device.

**End goal:** One queue per account, the same on every device, that can move between devices mid-track. Playback is gapless and normalized, and transcodes only when it has to. [`queue.md`](requirements/queue.md), [`playback.md`](requirements/playback.md), and [`realtime.md`](requirements/realtime.md) are met.

### Stage 10 — Offline & PWA

- Build search, sort, and queue rules into `core-wasm`, with a parity test suite against the native build.
- A web worker holds the catalog, synced from the change feeds, with a durable command queue for optimistic actions.
- A service worker, an installable PWA, downloads with a storage budget, choosing between local and stream, and reconciliation.

**Open questions:**
1. **The on-device catalog and index format**, within the [`performance.md` §6](requirements/performance.md#6-client-footprint) goals ([`design/general.md` §11](design/general.md#11-open-decisions) #2).
2. Where downloaded audio lives (Cache API, OPFS, or IndexedDB), and what iOS's storage limits allow ([`offline.md` §11](requirements/offline.md#11-platform-parity)).
3. How long change-feed tombstones are kept, and what triggers a full resync.
4. The format of the command queue, and how offline-created entities get server IDs (playlists already use an Idempotency-Key).
5. Whether album changes in the feed carry their tracks (carried over from Stage 1).
6. Background downloading in a PWA, especially on iOS.

**End goal:** The web app behaves the same with or without a network, apart from what audio can play and what there is to search. [`general.md` §3.4–3.5](requirements/general.md#34-optimistic-by-default), [`offline.md`](requirements/offline.md), [`search.md` §2](requirements/search.md#2-where-search-runs), and [`libraries.md` §5](requirements/libraries.md#5-staying-current) are met.

### Stage 11 — Listening data & recommendations

- Listening history, statistics, period summaries, clearing, and export.
- Add the sound description to the existing analysis pass. Start this early, because filling it in across a library takes days.
- Similarity, radio and endless play, generated mixes, rediscovery, and similar items on entity pages. Similarity data travels with the offline catalog.

**Open questions:**
1. **The feature extractor**: which sonic features to measure, and how they're stored ([`design/scanning.md` §16](design/scanning.md#16-open-decisions) #3).
2. **The similarity model**: how sound and behaviour are weighed, and how it stays stable from day to day.
3. How similarity data is compacted enough to ship with the catalog.
4. How often mixes regenerate, how many there are, and how they're named.
5. How clearing history removes its influence from statistics and recommendations ([`analytics.md` §9](requirements/analytics.md#9-privacy-and-control)).
6. How period summaries are presented.

**End goal:** Users can look back on what they played, and get back into their library without browsing. [`analytics.md`](requirements/analytics.md), [`recommendations.md`](requirements/recommendations.md), and generated playlists ([`playlists.md` §8](requirements/playlists.md#8-saving-a-generated-playlist)) are met.

### Stage 12 — Open plugin surface

- External content sources: search sections, browsing, mixed queues, playlist entries, and history marking.
- Plugin UI surfaces and actions. Installing and updating by URL, with a review step for permissions. Per-plugin timings, failures, and health for admins.
- More hooks and imports as needs arise: user actions, scan problems, decoded audio, notifications (`design/hooks.md` §3, `design/imports.md` §3).

**Open questions:**
1. **How plugin UI is described**, so both clients can render it natively ([`design/general.md` §11](design/general.md#11-open-decisions) #4).
2. **External identity.** The direction is identifiers that plugins compute on import, matched by the core. Storing them needs a deliberate amendment to [`general.md` §3.2](requirements/general.md#32-the-scanner-is-the-only-ingestion-path). It also needs guardrails: weak identifiers never chain, and durations are sanity-checked.
3. Storing the external entities users interact with, given that [`plugins.md` §10.1](requirements/plugins.md#101-it-is-not-library-content) says nothing is stored.
4. Whether users must opt in, or at least be told, before their searches go to an external source (`design/exports.md` §5).
5. How long a plugin's section or search may take before it is dropped. [`performance.md`](requirements/performance.md) no longer sets numbers, so decide where this limit lives.
6. Permissions for user-action hooks: one per kind of personal data, or one per kind of action.
7. Whether plugins can call each other, and under whose grants.

**End goal:** A plugin can add a new kind of feature, including its own pages and music the user doesn't own, without a core release. [`plugins.md`](requirements/plugins.md) is fully met.

### Stage 13 — Production readiness

- A multi-arch production image and compose file, startup config validation, migrations that are safe to interrupt, a health endpoint, and docs for reverse proxies, backups, and upgrades.
- A benchmark harness and library generator (`bench/`). Measure on low-power hardware such as a Pi and on a typical desktop, and publish the results.

**Open questions:**
1. **The project license** ([`design/general.md` §11](design/general.md#11-open-decisions) #5).
2. How a backup taken from a running server stays valid with SQLite's WAL.
3. The arm64 build, including `aws-lc-sys`, which hasn't been tested in a container yet.
4. Which reverse proxies the docs give examples for, and the downgrade policy.
5. Whether to set concrete numeric performance targets once there's real data.

**End goal:** Someone can install Jewelcase with `docker compose up`, upgrade it by changing the image tag, and check the published performance figures for themselves. [`deployment.md`](requirements/deployment.md) and [`performance.md`](requirements/performance.md) are met, and v1 of the web client is releasable.

### Stage 14 — Android

- A native Kotlin app on the same public API, feeds, and shared core ([`design/general.md` §7.2](design/general.md#72-android)), with background audio, system controls, and durable downloads.

**Open questions:**
1. How the shared core is bound into Kotlin (UniFFI, or hand-written JNI).
2. The audio stack and how it does gapless playback (for example Media3/ExoPlayer).
3. How plugin UI surfaces render natively, following Stage 12's format.

**End goal:** Every feature reaches Android, with no reduced version. [`general.md` §4](requirements/general.md#4-target-platforms), [`playback.md` §7](requirements/playback.md#7-system-integration), and [`offline.md` §11](requirements/offline.md#11-platform-parity) are met.
