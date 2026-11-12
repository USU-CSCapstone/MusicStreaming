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
3. Whether to keep the denormalized counts, or compute them on read ([`design/database.md` §11](design/database.md#11-open-questions)).
4. Tracks with no disc tag are stored as disc `0` and shown as `null`. Confirm or change this ([`design/database.md` §11](design/database.md#11-open-questions)).
5. The change feed names an album, but not its tracks, when the album's title, artists, or image change, even though track summaries embed the album. Decided with the offline catalog (Stage 10): it does not, because a device's tables are normalized.

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
1. Replaying the results of commands that have already finished.

**End goal (reached):** The server knows every device, which one is playing, and where it is in the track, and can route commands to it. The server side of [`realtime.md` §1–2, §4–6, §8](requirements/realtime.md) and [`users.md` §4](requirements/users.md#4-sessions--devices) is met.

---

## Remaining

### Stage 6 — Track identity

- Replace path-keyed identity with identity that follows content and survives moves and retags. Do this before playlists, history, and downloads start to depend on track IDs.

**Open questions:**
1. **What identity reads from a file.** Hashing whole files is ruled out at scale. The candidates are audio-region sampling, tag-independent fingerprints, inode plus size, and MusicBrainz IDs ([`design/general.md` §11](design/general.md#11-open-decisions) #3).
2. How audio replacement is told apart from a new track.
3. How existing path-keyed IDs migrate without breaking play history, playlists, likes, or hidden tracks. Until then a moved or renamed file loses its place in every playlist, Liked Songs included, and comes back unhidden: the entry stays on the track left missing at the old path, which the playlist keeps, so the migration can carry it to the file's new track.

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
1. ~~How playlist undo works.~~ Decided: undo is the client's. An edit is undone by a further edit that reverses it, and a deletion by holding the `DELETE` back until its Undo toast is gone (`web/src/lib/deletions.svelte.ts`). The server keeps no trash.
2. The format of the complete playlist export ([`playlists.md` §9](requirements/playlists.md#9-export)).
3. How lyric matching and ranking fit into the core search index, without breaking parity with client search.
4. How long recent searches are kept ([`search.md` §6](requirements/search.md#6-recent-searches)).
5. How "near misses" are chosen when a search finds nothing.
6. What a playlist keeps of an external entry: probably a display snapshot the playlist owns (`design/exports.md` §5, Stage 12).

**End goal:** Everything the server holds is reachable and navigable in the web app, and users keep their own playlists. [`conventions.md`](requirements/conventions.md), [`tags.md`](requirements/tags.md), [`search.md` §1, §3–8](requirements/search.md), and [`playlists.md`](requirements/playlists.md) are met, except for generated playlists (§8).

### Stage 9 — Queue, playback & Shift

- A server-owned queue with the Manual Queue, Context Queue, and History. Seeded shuffle in `crates/core`, repeat, and kept sessions. Move player state onto the queue.
  - **Built:** all of this, online: the core's queue rules, the server's queue, kept queues and player state on items, and the web's queue panel, player and queueing ([`design/queue.md`](design/queue.md), [`design/web.md`](design/web.md#queue-worker)). Views start and queue by recipe (album, artist, songs, search and playlist, with their tag filters), and the playlist and track sorts are the core's. Endless play is not; kept queues work offline (Stage 10).
- The web client side of realtime: connection, progress, a device picker, and remote control. Then Shift, waking, and per-device volume.
- Transcoding with a cache and seekable streams. Gapless playback, normalization, crossfade, silence skipping, and failure handling.

**Open questions:**
1. **Audio delivery and gapless in the browser**, above all in an installed iOS PWA ([`design/general.md` §11](design/general.md#11-open-decisions) #1). Spike this first, because it decides the stream format.
2. The transcode codec and bitrate for each quality tier, how client capabilities are negotiated, and how the transcode cache is sized and evicted.
3. How seeking works inside a transcoded stream.
4. ~~How a queue over a context of hundreds of thousands of tracks is stored and synced.~~ **Decided:** a few spans over frozen lists, each list stored once per account under a server-computed hash and fetched by it, so state and traffic grow with edits, not tracks ([`design/queue.md`](design/queue.md), [`design/database.md` §6](design/database.md#6-personal-data)).
5. ~~How "the user adjusted this queue" is detected.~~ **Decided:** `adjusted` is set by any add, remove or move, and by removing or re-queueing a play, and is never cleared, so a start keeps the queue if it is set and deletes it otherwise ([`design/queue.md` §5](design/queue.md#5-queue-state), [`queue.md` §6](requirements/queue.md#6-sessions)).
6. Silence skipping needs silence detection. It has to come from the existing analysis pass, never a second one.
7. Waveform bin count and shaping ([`design/scanning.md` §16](design/scanning.md#16-open-decisions) #2).
8. Normalization headroom: how quiet tracks are raised without clipping.
9. Whether one browser tab or several can play per device.

**End goal:** One queue per account, the same on every device, that can move between devices mid-track. Playback is gapless and normalized, and transcodes only when it has to. [`queue.md`](requirements/queue.md), [`playback.md`](requirements/playback.md), and [`realtime.md`](requirements/realtime.md) are met.

### Stage 10 — Offline & PWA

Stage 10 is two plans. The first makes the catalog, search and queue building work offline and makes the app installable. The second adds audio and the writes that survive a closed app.

#### Part 1: the offline catalog and the installable app ✅ (built)

- **The device keeps the whole catalog in SQLite** ([ADR 0008](adr/0008-sqlite-device-catalog.md), [`design/general.md` §3.1](design/general.md#31-the-catalog-queries)). The catalog's read SQL moves into `crates/catalog`, which the server and the device's worker both run, with a test that every shared query gives identical JSON on both.
- **Sync.** A first sync imports a snapshot file the server builds per library, downloaded gzipped so a download cut short resumes, and checked by its hash as the device imports it after the download. After that a device pulls the library's change feed and the account's (playlists), told to pull by realtime hints on `hello` and on every change ([`design/database.md` §5](design/database.md#5-change-feeds)). Catching up is applied at the head, in slices of about 50 ms for a big one so the worker answers other reads between them, with reads of the library being applied sent to the server or held until it ends, so a reader never sees half of it. The device database is opened exclusively with a persistent journal, and a download first checks that the device has room for it. Each feed has positions of its own, sent with the epoch of the server's run, so a restored backup refuses a device ahead of it.
- **The catalog worker** is owned by one tab at a time. The owner publishes what it holds, every tab reads that state, and a request is answered only to the tab that asked ([`design/web.md`](design/web.md#catalog-worker)). Pages, live updates and search read through it, by the rules of [`design/general.md` §7.1](design/general.md#71-web). An open page updates in place when the copy changes in a kind of row it shows, and is not rebuilt when nothing it shows changed.
- **Search on the device** builds the core's own index from the device database inside the catalog worker, and a golden-file test checks that it ranks like the server. The queue's rules, the list hash and the track-list comparator (`sort_rows`) run in `core-wasm`.
- **The queue builds on the device**: the list hash moves into the core, and a start or add of a recipe resolves, hashes and attaches on the device, downloading the server's list only on a mismatch ([`design/queue.md` §7](design/queue.md#7-where-it-is-built-and-what-is-not-yet)).
- **The installable app**: a manifest, a service worker that precaches the build, a per-account image cache with ThumbHash placeholders, one connectivity state (online, unreachable, offline or working offline) with an indicator and a per-device *Work offline* switch, and a Storage section on the account page ([`design/web.md`](design/web.md#the-installable-app)).
- **One account's data per device.** Logging out, a session the server ends and another account signing in wipe the catalog, art, lists and sync log; logging out offline wipes at once and tells the server when it can ([`requirements/offline.md` §9](requirements/offline.md#9-download-lifecycle)).
- **Kept queues on the device.** The account's kept queues, their states and lists are kept on the device, filled from the server and by the starts and restores made on it, so the Kept tab lists them and restore and delete work offline ([`design/queue.md` §7](design/queue.md#7-where-it-is-built-and-what-is-not-yet)).
- **Writes that survive a closed app.** Every write (plays, likes and hides, playlist creates, edits and deletes, the queue's starts, edits and restores, and kept queues' deletions) is a mutation in one per-account IndexedDB log, shown on the device at once and sent in order when the app is next online, each applied once by the server under its idempotency key ([`design/web.md`](design/web.md#the-sync-framework), [`design/queue.md` §7](design/queue.md#7-where-it-is-built-and-what-is-not-yet)).
- **Left for Part 2 or later**: the feed horizon, lyric search on the device, recent searches offline (they need the durable writes below), and the web app's icons, which are a placeholder until the project has artwork.
- **Accepted until Part 2**: tombstones never age out; a device offline before its first sync says the library is not on it yet; nothing plays offline; a logged mutation can be lost to a browser that clears script-written storage; mutations set aside after repeated failures are not shown.

**Open questions:**
1. ~~The on-device catalog and index format~~ Answered: SQLite on the device, with the catalog's SQL shared with the server. A 500,000-track copy is about 360 MB, within [`performance.md` §6](requirements/performance.md#6-client-footprint)'s goal of well under 0.1% of the music.
2. How long change-feed tombstones are kept, and what triggers a full resync. Carried: nothing is dropped and no horizon exists. A device whose cursor the server refuses (`410`, including one from beyond a restored backup) downloads the snapshot again, keeping its copy until the new one is in, and the same happens when a catch-up would change more than a tenth of a library.
3. ~~Whether album changes in the feed carry their tracks~~ Answered: no. A device's tables are normalized, so a track's album, artist and tag names are joins and an album change sends only the album.

#### Part 2: downloads and offline writes (next)

- Downloads with a storage budget, choosing between local and stream, a download as a standing instruction, and reconciliation ([`requirements/offline.md`](requirements/offline.md)).
- **Durable offline writes**: a way to see or retry what the log set aside. Plays, likes and hides, playlist creates, edits and deletes, and queue edits are built (*The sync framework*).
- **The queue works fully offline.** The active queue, a start made offline and the kept queues are kept and sent on reconnect, and a restore works offline (Part 1). IndexedDB holds lists, so they stop depending on Cache Storage. A parity suite compares what a device builds with the server's resolves, since sorts a device cannot reproduce (personal-data sorts, search ranking) always travel as lists.
- The account feed grows to carry plays, queues and kept queues as the writes need them.

**Open questions:**
1. Where downloaded audio lives (Cache API, OPFS, or IndexedDB), and what iOS's storage limits allow ([`offline.md` §11](requirements/offline.md#11-platform-parity)).
2. How offline-created entities get server IDs (playlists already use an Idempotency-Key).
3. Background downloading in a PWA, especially on iOS.

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
7. How likes and hides weigh against listening behaviour: a like is a certain yes and a hide a certain no ([`recommendations.md` §2.2](requirements/recommendations.md#22-how-the-user-listens)), and a hidden track is never recommended or played by radio or endless play ([`conventions.md` §8](requirements/conventions.md#8-hidden-tracks)).

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
8. **Plugins setting likes and hides**, such as importing a user's Last.fm loves into Liked Songs. Plugins hear likes and hides ([`plugins.md` §8](requirements/plugins.md#8-events)) but cannot set them. Setting them would be a new path into personal data, which needs requirements of its own: whose consent, and whether the user sees what a plugin liked for them.

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
