# Imports Design

## Overview
What a plugin can call on the host. This file lists the host interfaces that are built and the ones proposed, each with what it does, why a plugin needs it, and what it needs granted. It builds on [`requirements/plugins.md`](../requirements/plugins.md) and on [`plugins.md`](plugins.md), which covers how a run executes. Where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **the fast way to do something is the easy way** ([`requirements/plugins.md` §2.1](../requirements/plugins.md#21-the-system-adds-nothing)). Most slow plugins are slow because the host offered no better access pattern than a slow one. So when an import is missing, the cost is more than inconvenience: authors work around it, and the workaround is slow.

Its companions are [`hooks.md`](hooks.md), covering what runs a plugin, and [`exports.md`](exports.md), covering what the host can call on a plugin. Everything in §3 is proposed, not decided, except what it marks as built.

---

## 1. Built Imports

**Seven interfaces, all in [`wit/plugin.wit`](../crates/plugins/wit/plugin.wit) at `0.2`.** Every import checks its own permission in the host. Every import stays linked when its permission is declined and answers an error instead, so optional permissions work ([`plugins.md` §7](plugins.md#7-running-plugins)). WASI is linked with nothing granted: no preopens, sockets, or inherited stdio.

| Interface | Functions | Needs | Limits |
|---|---|---|---|
| `host` | `granted`, `log` | — | 1,000 log lines per run |
| `library` | `tracks`, `get-tracks`, `albums`, `artists` | `libraryRead` | 500 per page; pages continue from the last ID |
| `files` | `roots`, `list`, `read` | `libraryRead` (`roots`: any library permission) | 10,000 entries per list; 8 MB per read |
| `files` | `write` (`create`) | `libraryAdd` | 32 MB, written whole or not at all |
| `files` | `write` (`replace`), `rename`, `delete` | `libraryChange` | 32 MB; a folder is deleted only when empty |
| `http` | `send` | `network`, to declared destinations only, redirects included | 16 MB bodies, 15 s per request, 5 redirects; a shared pace and cache (below) |
| `state` | `get`, `set`, `delete` | — (per plugin and library) | 512 B keys, 1 MB values |
| `settings` | `get` | — | Library value, else server-wide value, else the default, plus the connected user's personal settings |

The limits on every run are 64 MiB of memory, 60 s of compute, and 5 minutes of wall clock.

**`http.send` shares a pace and a cache across every plugin** ([`plugins.md` §7](plugins.md#7-running-plugins)). A network request declares `rateLimits` per destination, and the strictest declared pace applies to every request to that host. A `429`, or a `503` with `Retry-After`, pauses that host for everyone. A GET is answered from a fresh reply already fetched for the same library, URL, and headers, and a stale one is checked with its `ETag` or `Last-Modified`.

---

## 2. Rules Every Import Follows

- **Each one arrives with its permission** ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)). An import that reaches something new either sits under an existing permission that already covers it, or brings its own. Nothing reachable is left ungoverned.
- **Bound to the run's library** in the store, never taken as a parameter. A plugin cannot name a library it was not enabled for ([`plugins.md` §2](plugins.md#2-execution-model)).
- **Person-scoped imports reach only the user the run is for**, and only one who connected the plugin. No import lists users ([`requirements/plugins.md` §11](../requirements/plugins.md#11-isolation--boundaries)).
- **Linked when declined, refusing when called**, as §1 describes.
- **Bulk by default.** An import that returns entities takes a list or a page, never only one at a time.
- **Nothing writes to the database.** Everything a plugin changes in a library goes through files and the scanner ([`requirements/general.md` §3.2](../requirements/general.md#32-the-scanner-is-the-only-ingestion-path)).

---

## 3. Proposed Imports

### 3.1 Requirements Not Yet Met

These close gaps where [`requirements/plugins.md`](../requirements/plugins.md) already promises something the contract cannot do.

#### Shared Cache and Coordinated Rate Limits

**Built** (§1, [`plugins.md` §7](plugins.md#7-running-plugins)). The design moved in two ways from what was first proposed here:

- **Credentials did not need marking.** The proposal was to cache a request carrying credentials only for its own plugin. Instead the cache key is the library, the URL, and every header sent. A reply fetched with a credential is then reused only by a request carrying the same credential, wherever the credential is, so the host never has to recognise one.
- **The cache is per library, not server-wide.** Whether a reply is already cached, measured by how fast it comes back, would tell a plugin what another library holds. Plugins in one library still share.

What remains:

- **`cache.get` and `cache.set`**, for derived results that are not HTTP replies, if the HTTP cache proves not to be enough. Scoped to the plugin.
- **Merging requests in flight.** Two plugins asking the same question at the same moment both ask, because the second misses before the first reply is kept.

#### Progress and Checkpoints

**Long jobs are bounded and resumable ([§7](../requirements/plugins.md#7-working-with-the-library)), and nothing tells a plugin its run is about to end.**

- **`host.progress(done, total)`** shows admins real progress rather than a single line once the run ends.
- **`host.time-remaining()`** returns the smaller of compute and wall clock left. A plugin can then save its place in `state` and stop cleanly, rather than being cut off.
- **A structured outcome** that says "more to do" belongs in the export ([`exports.md` §3.4](exports.md#34-structured-outcomes)), so the host can schedule the next run.
- **Needs** nothing.

#### Streaming Writes and Downloads

**A whole file passes through the plugin's memory today, up to 32 MB.** An acquisition plugin bringing in a 500 MB FLAC cannot, and should not have to ([`plugins.md` §7](plugins.md#7-running-plugins)).

- **`http.download(request, root, path, mode)`** sends the request and streams the response body straight into the file. The bytes never enter the plugin. Atomicity and the `create`/`replace` rules are the same as `files.write`.
- **`files.open-write(root, path, mode)`** returns a handle the plugin appends to and then commits or abandons. This covers files the plugin produces itself rather than downloads. It appears whole on commit, and the scanner reads it afterwards.
- **Needs** `network` and `libraryAdd` or `libraryChange` for the download. The handle needs the library permission alone.

#### Bulk Reads

**A full pass over 500,000 tracks costs 3.2× the host's own scan**, because the canonical ABI calls into the guest to allocate each string ([`plugins.md` §3](plugins.md#3-data-access), open question 3).

- **A column layout:** one buffer of titles plus offsets, one of artists, and so on, for passes that read one or two fields across the whole library.
- **Needs** `libraryRead`.

### 3.2 Reading the Library

**[§4.1](../requirements/plugins.md#41-what-can-be-asked-for) says `libraryRead` covers "its catalog, artwork, lyrics, and audio".** Only the catalog is reachable, and only by paging through it.

| Import | Why |
|---|---|
| `track-by-path(root, path)` | A plugin that just wrote a sidecar, or found a file through `files.list`, needs to know which track it belongs to. |
| `get-albums(ids)`, `get-artists(ids)` | `get-tracks` exists; albums and artists are only reachable by paging. The album and artist hooks need these ([`hooks.md` §3.3](hooks.md#33-albums-changed-and-artists-changed)). |
| `album-tracks(album-id)`, `artist-albums(artist-id)` | Artwork and biography plugins think in albums and artists. Rebuilding those from a full pass is the slow access pattern §2.1 warns about. |
| `tracks-by-identifier(kind, values)` | Matching by ISRC or MusicBrainz ID, which is what [`requirements/tracks.md` §6](../requirements/tracks.md#6-classification--identifiers) stores identifiers for. |
| `search(query, limit)` | The core's own search over this library ([`requirements/search.md`](../requirements/search.md)). An external source, or a plugin mapping a ListenBrainz recommendation to an owned track, needs "which of my tracks is this", and should not have to build its own matcher. |
| `artwork(album-id)` | The image bytes, for a plugin that checks resolution before fetching a better one ([`requirements/albums.md` §5](../requirements/albums.md#5-artwork)). |
| `lyrics(track-ids)` | The lyrics themselves, not only `has-lyrics`, for translation or for converting plain lyrics to timed ones. |

- **A richer `track` record.** It carries MusicBrainz IDs, genre, composer, and the other tags [`requirements/tracks.md`](../requirements/tracks.md) stores so that plugins can match reliably. Adding fields to a record is a contract change, so they should be added together, once.
- **All need** `libraryRead`.

### 3.3 Decoded Audio

**`audio.decode(track-id, sample-rate, channels)` returns PCM in chunks**, decoded by the host with the `ffmpeg` crate the scanner already uses. ReplayGain ([`requirements/playback.md` §5](../requirements/playback.md#5-loudness)), AcoustID fingerprinting, and BPM or key detection all need raw samples. Without this, every such plugin bundles its own decoder for every format, in Wasm, at Wasm speed.

- **Runs as background work**, yielding to listeners like the scanner ([`requirements/plugins.md` §2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)). Decoding counts against the plugin's attributed cost, even though the host does it.
- **Needs** `libraryRead`, which already covers audio.

### 3.4 For the User a Run Is For

**These exist only in runs for one connected user**, such as `played` and the proposed person-scoped hooks ([`hooks.md` §3](hooks.md#3-proposed-hooks)). Elsewhere they refuse.

- **`listening.history(after, limit)`**: that user's plays in this library. It lets a plugin backfill a newly connected account with years of history. **Needs** `listeningActivity`.
- **Per-user `state`.** A Last.fm session key, or the last play sent, belongs to a person rather than a library. Today the plugin builds keys like `user:<id>:…` itself, and must then be trusted to delete them. A store scoped by the host can be deleted by the host when the user disconnects or deletes their account ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)). **Needs** nothing.
- **`host.user()`**: an opaque, stable ID for that user, never their name or email. **Needs** nothing.

### 3.5 Telling People Things

**`notify(audience, message)` for "health checks, notifications"**, which [§1](../requirements/plugins.md#1-an-open-surface) names as plugins people will build.

- **Audiences:** admins, or the user a run is for. Never "every user", which would turn a plugin into a broadcast channel nobody approved.
- **Needs a new permission, `notify`**, per plugin. Notifications reach people, and an admin may reasonably refuse that.
- **Rate-limited by the host**, so a plugin stuck in a loop cannot flood anyone.

### 3.6 Not Imports

- **Writing tags belongs in a guest SDK, not the host.** `lofty` compiles to WASI, and `files.write` with `libraryChange` already allows it. A host import would grow the contract for something a library does just as well, and plugins would wait on core releases to support a new tag field.
- **Calling another plugin.** One MusicBrainz-matching plugin serving the artwork and biography plugins would save work. But it needs a design for whose grants apply, and for whether data that crosses between plugins is a leak. Deferred until there is a concrete need ([§4](#4-open-questions)).
- **Anything that writes to the database.** See §2.

---

## 4. Open Questions

1. **Who declares a rate limit.** Each plugin declares its own and the strictest wins, so a careless plugin can only make things slower, never faster. A host-kept list for well-known services would be curation the project said it does not do ([`requirements/plugins.md` §4.4](../requirements/plugins.md#44-who-is-trusted)). A declaration is remembered until restart, even after its plugin is uninstalled, which errs the same way.
2. **Cross-plugin calls** (§3.6): whether they happen at all, and under whose grants.
3. **Imports checked at install.** [`plugins.md` §6](plugins.md#6-packaging) proposes refusing a plugin that imports network access it did not request. Each import added here makes that check more valuable, and it is not built.
