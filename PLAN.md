# Development Plan

How Jewelcase gets from where it is to everything in [`requirements/`](requirements/). This file is about **order and status**. What the system must do lives in the requirements, and how it does it lives in [`design/`](design/) and [`adr/`](adr/).

---

## How firm is each part

| Level | What it covers | How it changes |
|---|---|---|
| **Fixed** | The core principles ([`general.md` §3](requirements/general.md#3-core-principles)) and accepted ADRs | Only by a deliberate amendment, or a new ADR that supersedes the old one |
| **Built** | Requirements the code already meets | Amend the requirement first, then plan the migration or rework |
| **Open** | Requirements nobody has built yet, and the parts of `api/openapi.yaml` the server doesn't serve yet | Adjust freely, ideally at the start of the stage that builds them |

Anything not listed as Built below is Open.

---

## Process

1. **Start a stage by re-reading its requirement files.** Settle any changes to its Open requirements first, then update `api/openapi.yaml`, then write code. The requirements and the spec never fall behind the code.
2. **Make one small, reviewable change at a time**, one branch and one PR each. Changes stack when they depend on each other.
3. **Close a stage by checking its end goal**: move its requirements to Built in the status table, and write down anything deferred.
4. **Test every change at its boundary.** Library and account isolation are checked on each new endpoint ([`general.md` §3.6](requirements/general.md#36-libraries-are-the-isolation-boundary)), and new work is checked against the goals in [`performance.md`](requirements/performance.md), on low-power hardware once the benchmark harness exists (Stage 8).

The stages are in dependency order. Stages that don't depend on each other can run in parallel, for example 2 with 3, or 6 with 7.

---

## Status

| Area | Status | Gaps |
|---|---|---|
| [Scanning](requirements/scanning.md) | Built | Sound description for recommendations; admin views of problems, missing tracks, and duplicates |
| [Tracks](requirements/tracks.md), [albums](requirements/albums.md), [artists](requirements/artists.md) | Built (API) | Related artists; the web UI doesn't fully cover the requirements yet |
| [Libraries](requirements/libraries.md) | Partial | Scoping is built. No admin management, and the only library comes from `JEWELCASE_MUSIC` |
| [Users](requirements/users.md) | Partial | Setup, login, logout, sign-in limits, and roles are built. No invites, password change or reset, devices UI, or account management |
| [Search](requirements/search.md) | Partial | Server matching is built. No sections or top result, no lyrics, no recent searches, no on-device search |
| [Tags](requirements/tags.md) | Partial | Stored, but no tag pages or filters |
| [Playback](requirements/playback.md) | Partial | Direct streaming with range requests. No transcoding, gapless, normalization, or failure handling |
| [Realtime](requirements/realtime.md) | In progress | Devices, WebSocket, player state, and remote commands are on unmerged branches. No web client side or Shift yet |
| [Plugins](requirements/plugins.md) | Mostly built | Host, permissions, files, HTTP, state, settings, and hooks are built. More hooks are on unmerged branches. No external content or UI surfaces |
| [Analytics](requirements/analytics.md) | Partial | Plays are recorded. No history, statistics, or summaries |
| [Queue](requirements/queue.md) | Not started | There's a client-only stepper |
| [Playlists](requirements/playlists.md) | Not started | |
| [Offline](requirements/offline.md) | Not started | `core-wasm` is a placeholder |
| [Recommendations](requirements/recommendations.md) | Not started | |
| [Deployment](requirements/deployment.md) | Partial | Dev containers and env config only. No production image |
| [Performance](requirements/performance.md) | Not started | No benchmark harness |
| Android | Not started | |

---

## Stage 0 — Foundation *(done)*

Rust server, SQLite, and a SvelteKit SPA ([ADRs 0001–0006](adr/)). Scanner with identity, sidecars, watching, loudness, and waveforms. Browse, search, image, lyric, and audio endpoints. Owner setup, login, and library scoping. A Wasmtime plugin host with permissions and hooks, play recording, and two reference plugins.

**End goal (reached):** A single library scans and can be browsed, searched, and played in the web app by signed-in users. Plugins can enrich it and scrobble.

## Stage 1 — Land in-flight work

- Merge the realtime chain (`api-list-devices` … `control-playback-http`) and build its web client: connection, progress, and the device picker.
- Merge the plugin hook chain (`pluginUpgrade` … `albumArtistHooks`) and the external-content spec work (`cleanup-playlists-api-reference`).

**Open:** Player state temporarily carries `libraryId` + `trackId` until the server queue exists (Stage 4). The plugin-identifier amendment to [`general.md` §3.2](requirements/general.md#32-the-scanner-is-the-only-ingestion-path) is still undecided.

**End goal:** `main` has live device lists, one player per account, and remote play, pause, skip, and seek from any signed-in tab. [`realtime.md` §1–2, §4 (playback), §5–6, §8](requirements/realtime.md) and [`users.md` §4](requirements/users.md#4-sessions--devices) are addressed.

## Stage 2 — Accounts & administration

- Invites, account suspension and deletion, ownership transfer, password change and reset, device sign-out, the audit log.
- Library management: create, rename, roots, exclude rules, delete, access grants, and a health view.
- Admin views for scan progress, problems, missing tracks, and duplicates.

**Open:** Data export format ([`users.md` §9](requirements/users.md#9-account-lifecycle)), and the `session_revoked` signalling left open in the auth work.

**End goal:** A household can run one server with several libraries, with no shell access needed. [`users.md`](requirements/users.md) and [`libraries.md` §1–4](requirements/libraries.md) are met, as are the admin surfaces of [`scanning.md` §8–10](requirements/scanning.md#8-missing-files).

## Stage 3 — Browsing, search & playlists

- Tag pages and filters, the [`conventions.md`](requirements/conventions.md) sorting and actions everywhere, a library switcher, discography versus appearances in the UI, and drawn placeholders.
- Search results with a top result, sections, lyric matches, scoping, and recent searches.
- Playlists: CRUD, bulk and non-contiguous edits, version-checked edits, undo, artwork, and export.

**End goal:** Everything the server holds is reachable and navigable in the web app, and users keep their own playlists. [`conventions.md`](requirements/conventions.md), [`tags.md`](requirements/tags.md), [`search.md` §1, §3–8](requirements/search.md), and [`playlists.md`](requirements/playlists.md) are met, except for generated playlists (§8).

## Stage 4 — Queue, playback & Shift

- A server-owned queue, with the Manual Queue, Context Queue, and History. Seeded shuffle in `crates/core`, repeat, endless-play hooks, and kept sessions.
- Rework player state onto the queue. Then Shift, waking, and per-device volume.
- Transcoding with a cache and seekable streams. Gapless playback, normalization, crossfade, failure handling, and Media Session controls.

**Open:** Audio delivery and gapless playback in the browser, especially an installed iOS PWA ([`design/general.md` §11.1](design/general.md#11-open-decisions)). Spike this before committing to a stream format.

**End goal:** One queue per account, the same on every device, that can move between devices mid-track. Playback is gapless and normalized, and transcodes only when it has to. [`queue.md`](requirements/queue.md), [`playback.md`](requirements/playback.md), and [`realtime.md` §3, §7](requirements/realtime.md#3-shifting-control-and-waking) are met.

## Stage 5 — Offline & PWA

- Move search, sort, and queue rules into `core-wasm`, with a parity test suite against the native build.
- A web worker holding the catalog, synced from the change feeds. A durable command queue for optimistic actions.
- A service worker, an installable PWA, downloads with a storage budget, choosing between local and stream, and reconciliation.

**Open:** The on-device catalog and index format, within the [`performance.md` §6](requirements/performance.md#6-client-footprint) footprint goals ([`design/general.md` §11.2](design/general.md#11-open-decisions)).

**End goal:** The web app works the same with or without a network, apart from what audio can play and what there is to search. [`general.md` §3.4–3.5](requirements/general.md#34-optimistic-by-default), [`offline.md`](requirements/offline.md), [`search.md` §2](requirements/search.md#2-where-search-runs), and [`libraries.md` §5](requirements/libraries.md#5-staying-current) are met.

## Stage 6 — Listening data & recommendations

- Listening history, statistics, period summaries, clearing, and export.
- Add the sound description to the existing single analysis pass. Do this early, because filling it in across a library takes days.
- Similarity, radio and endless play, generated mixes, rediscovery, and similar items on entity pages. Similarity data rides along with the offline catalog.

**Open:** How similarity is computed and how sound and behaviour are weighed. All of [`recommendations.md`](requirements/recommendations.md) can still be adjusted.

**End goal:** Users can look back on what they played, and get back into their library without browsing. [`analytics.md`](requirements/analytics.md), [`recommendations.md`](requirements/recommendations.md), and generated playlists ([`playlists.md` §8](requirements/playlists.md#8-saving-a-generated-playlist)) are met.

## Stage 7 — Open plugin surface

- External content sources: search sections, mixed queues, playlist entries, and history marking.
- Plugin UI surfaces, installing and updating by URL, and per-plugin timings and health for admins.

**Open:** How plugin UI is described ([`design/general.md` §11.4](design/general.md#11-open-decisions)), external identity matching, and storing the external entities users interact with ([`plugins.md` §10.1](requirements/plugins.md#101-it-is-not-library-content)).

**End goal:** A plugin can add a new kind of feature, including its own pages and music the user doesn't own, without a core release. [`plugins.md`](requirements/plugins.md) is fully met.

## Stage 8 — Production readiness

- A multi-arch production image and compose file, startup config validation, safe migrations, a health endpoint, and docs for reverse proxies and backups.
- A benchmark harness and library generator (`bench/`). Measure on low-power hardware such as a Pi as well as a typical desktop, and publish the results. Concrete numeric targets can be set here, once there's real data.

**Open:** The project license ([`design/general.md` §11.5](design/general.md#11-open-decisions)).

**End goal:** Someone can install Jewelcase with `docker compose up`, upgrade it by changing the image tag, and check the published performance figures for themselves. [`deployment.md`](requirements/deployment.md) and [`performance.md`](requirements/performance.md) are met, and v1 of the web client is releasable.

## Stage 9 — Android

A native Kotlin app on the same public API, feeds, and shared core through native bindings ([`design/general.md` §7.2](design/general.md#72-android)), with background audio, system controls, and durable downloads.

**End goal:** Every feature reaches Android with no reduced version. [`general.md` §4](requirements/general.md#4-target-platforms), [`playback.md` §7](requirements/playback.md#7-system-integration), and [`offline.md` §11](requirements/offline.md#11-platform-parity) are met.
