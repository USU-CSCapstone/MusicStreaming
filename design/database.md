# Database Design

## Overview
The SQLite schema behind `api/openapi.yaml`. Migrations are plain SQL in `crates/server/migrations/`, applied in order at startup (`design/general.md` §2). There is one file per domain, so each table's whole definition is in one place: libraries and scanning, accounts, the catalog, the change feeds, listening and searching activity, and plugins. Sections of the API not yet built add their tables as they are (§9).

---

## 1. Conventions

| | Rule |
|---|---|
| **IDs** | Random positive 63-bit integers as `INTEGER PRIMARY KEY`, generated in each `INSERT` by `random() & 0x7FFFFFFFFFFFFFFF`. A collision, about one in 10¹³ per insert at a million rows, fails its transaction rather than being retried. The API shows them as decimal strings, which clients treat as opaque. |
| **Timestamps** | Integer milliseconds since the Unix epoch, UTC, named `*_at`. |
| **Durations** | Integer microseconds, named `*_us`, matching the API. Playback positions and listen time are the exception: integer milliseconds named `*_ms` (`player_states.position_ms`, `plays.listen_time_ms`), because the API carries them in milliseconds (`positionMs`, `listenTimeMs`). |
| **Booleans** | `INTEGER` with `CHECK (x IN (0, 1))`. |
| **Indexes** | Partial and unique indexes, feeds (ending in `seq`, named `_feed`) and sweeps (`_age`) are named for their purpose: `tracks_missing`, `tracks_file`, `library_changes_feed`, `search_events_age`. Every other index is `<table>_by_<key>`, leaving out a leading `library_id`: `tracks_by_added`, `plays_by_user`. |
| **Tables** | `STRICT`, so a column holds only its declared type. Link tables are `WITHOUT ROWID`. |
| **Names** | A closed set of names is `CHECK`ed, such as `scans.trigger`. Plugin permission and hook names are not: they grow with the plugin contract, and Rust checks them (`design/plugins.md` §6). |
| **JSON** | Only for values never queried by field: settings, exclude patterns, labels, pass-through identifiers. Always `CHECK`ed as valid and of the right shape. |
| **Secrets** | Tokens and codes are stored as SHA-256 hashes, passwords as PHC strings. Nothing secret is stored readably. |

**Why random IDs.** Sequential IDs are shared across libraries, so a user who sees track 1000 and then track 1003 learns that tracks 1001 and 1002 exist somewhere they cannot reach. That breaks "inaccessible is indistinguishable from nonexistent" (`requirements/users.md` §10). Random 64-bit IDs stay as compact as sequential ones for the catalog on device, and reveal nothing. Internal-only tables, such as `login_attempts` and the feeds, keep ordinary sequential rowids.

**An ID a client chooses is keyed with its user.** A play keeps the `playId` its device chose, so it is counted once however often it is reported (`requirements/analytics.md` §6), and any other user can choose the same one, by chance or on purpose. So `plays` is keyed by `(user_id, id)`, `play_events` refers to a play by both, and one user's report never reaches another's play.

**A signed-out device keeps its token hash.** `devices.revoked_reason` marks it signed out, so a request with its token is answered `401 session_revoked` with the reason, which tells the device whether to keep its downloads (`requirements/offline.md` §9). Its row is deleted only by the device's own `logout` or with its account. Every sign-in makes a new random token, so a revoked one never authenticates again, and a suspended account's tokens, signed out or not, answer as unknown ones do (`requirements/users.md` §10).

---

## 2. Isolation by Construction

**Every content table carries `library_id`, and every link between content rows is a composite foreign key on `(library_id, id)`.** A track can only reference an album, root, or artist in its own library, and a link across libraries fails at insert (`requirements/general.md` §3.6). Deleting a library cascades through everything it holds, and removes nothing outside it.

**`plugin_grants` and `plugin_settings` keep what applies to a plugin everywhere as library 0**, which no library can have, since `libraries.id` is `CHECK`ed above zero. So neither can reference `libraries`, and a trigger on `libraries` deletes a library's rows in both with it.

**A composite foreign key cannot use `ON DELETE SET NULL`**, because it would clear `library_id` too, which is `NOT NULL`. So the links to an image, an album, or a root (`artists.image_id`, `albums.image_id`, `tracks.album_id`, `tracks.root_id`, `images.root_id`) take no action on delete, and such a row cannot be deleted while anything still refers to it. Deletes run in dependency order: an album goes only once no track is left on it, and anything that deletes images must delete only those nothing refers to (nothing deletes images yet).

---

## 3. Derived Data the Scanner Maintains

The scanner is the only writer of content (`requirements/general.md` §3.2), so it keeps derived values current in the same transaction as the change that affects them. Reads, which dominate, never recompute them.

| Derived | Stored on |
|---|---|
| Counts and durations | `libraries`, `albums`, `artists`, `tags` |
| Sort keys, computed by the shared core | Every sortable table. Tracks also copy their album's and first artist's sort key. |
| Album fields resolved from tracks — release date, disc and track totals, loudness | `albums`, `album_discs` |
| Displayed names: each name's most-used spelling | `artists.name`, `albums.title`, `tags.name` |
| Album artists, resolved from each file's album-artist credits | `album_artists` |
| Album and artist genres | `album_tags`, `artist_tags` |

**Sort keys are copied onto tracks** because browsing pages with keyset pagination over one index (§4), and sorting tracks by artist or album would otherwise need a join at every page.

**Identity is a folded key; the displayed name is the most-used spelling.** Each artist credit is split on semicolons, trimmed of outer whitespace, and Unicode case-folded into `artists.name_key`, which decides identity. The folding happens in the server, because SQLite's `NOCASE` covers ASCII letters only. Albums follow the same rule: `title_key` is the title trimmed and case-folded, `artists_key` is the album artists' `name_key`s sorted and joined with `;`, and the two together are unique. Tags fold case and spacing into `tags.name_key` (`requirements/tags.md` §4).

The displayed name — `artists.name`, `albums.title`, `tags.name` — is the spelling most files use, so it follows the collection. Each track records how its file writes every name: `track_artists.artist_name`, `track_album_artists.artist_name`, `tracks.album_title`, and `track_tags.tag_name`. The scanner recounts from these whenever tracks change, without rereading files, and breaks ties by the spelling's bytes so a tie never flips the name between scans.

**Library totals add up the albums.** Every track belongs to exactly one album (an unknown album for its album artists when its file names none), and an album's track count and duration include its missing tracks. So the library's `track_count` and `duration_us` are sums over its albums: in the same transaction the scanner first recomputes the albums a batch touched, and the rest are already current. That reads the library's albums rather than its tracks: about 5 ms warm and 35 ms cold at 500k tracks, against 60–80 ms warm and several hundred cold for summing the tracks. `album_count` and `artist_count` count rows.

**Per-file tags stay on the track.** Spellings, `tracks.track_total`, and `disc_total` keep what each file says, and resolution happens once, into `album_discs` and `albums`. When tags disagree, the resolution rule lives in one place.

---

## 4. Pagination

**Keyset, never `OFFSET`.** A cursor encodes the last row's sort columns and `id`, and the next page is `WHERE (sort, id) > (…) ORDER BY sort, id LIMIT n`. Every browse sort has an index ending in `id`, so page 5,000 costs what page 1 does (`requirements/conventions.md` §3). Ties are broken by a meaningful column before `id` wherever they are common, since random IDs would otherwise scatter them — albums from the same year sort by name, and tracks by release date keep album order.

**Filtered browsing picks its driving side by size.** A filtered page is either a walk of the sort index that checks each row against the filter, or a lookup of the filter's rows that are then sorted. Each is fast on one side and slow on the other: at 300k tracks, a 100-track genre sorted by name takes 0.8 ms from the tag and 27 ms walking the name index, while a 100k-track genre takes 60 ms from the tag and 10 ms walking. The server chooses using the counts it already stores (`tags.track_count`, `artists.track_count`, `albums.track_count`), rather than leaving it to the planner.

**`PRAGMA optimize` runs periodically**, so the planner has statistics. Without them, the small-genre walk above took 168 ms.

---

## 5. Change Feeds

- **A change row names the entity, never its contents.** Catching up reads each changed entity's current representation, so the feed and the data cannot disagree.
- **Coalesced on write.** Recording a change deletes that entity's earlier row, so each entity has at most one row, carrying its latest position. "Everything after cursor N" is then already the net effect.
- **Tombstones age out.** Deletions leave one row. Dropping those older than the retention window advances the feed's horizon, and older cursors answer `410`. Neither the aging out nor the horizon is built yet; each feed gains its horizon column with them.
- **Written in the same transaction** as the change itself.

---

## 6. Personal Data

**Listening statistics are read from `plays`**, never stored as a separate truth that could drift from them (`requirements/analytics.md` §2). None is served yet; each arrives with its section of the API, with whatever index it needs.

**Hooks that act for one user read their domain's outbox** (`design/general.md` §4), written by the same transaction as the activity:

- `play_events` holds each play's start and end, for `playing` and `played`, with a `kind` and one sequence. The feed index `play_events_feed` is keyed `(library_id, user_id, kind, seq)`, so each hook's due check and batch read one range of its own kind, as `library_changes_kind_feed` does for the change hooks. A position caught up on ends never steps over later starts, nor the reverse, though ends pile up when a device catches up after being offline. Stale starts are swept on each new play through `play_events_started_age`, a partial index on starts alone, so the sweep never reads an end.
- `search_events` names each search or forget, and keeps what a search found. Rows are written only for a user who shares their searches, and only then is the search index asked to count, so no one else's search pays for it.

**A user's connection to a plugin is one row of `plugin_connections`**, keyed by plugin and user and the same in every library: their personal settings, when they connected, and `searches_shared_at`, null until they share their searches with it. Sharing is a column of the connection rather than a table of its own, so it can never exist without one, and disconnecting deletes both. Settings are kept as entered, secrets included, because the plugin needs them; the API never returns the secrets (`design/plugins.md` §7). A connection goes with its user. It is keyed by the plugin's ID rather than referencing `plugins`, so it outlives an uninstall, as grants and settings do (`0006_plugins.sql`).

---

## 7. State and Cache

| In the database (`data/state/`) | Rebuildable (`data/cache/`) |
|---|---|
| Everything above | Resized images |
| Waveforms and loudness — analysis takes days to redo | Search indexes |
| | Transcodes |

**Every connection must enable `PRAGMA foreign_keys = ON`.** SQLite leaves foreign keys off by default, and §2 depends on them.

Migrations are the exception: they run with foreign keys off, because rebuilding a parent table with them on deletes or clears every reference to it. Each migration commits only if `PRAGMA foreign_key_check` finds no broken reference anywhere in the database. A rebuild creates the new table, copies the rows, drops the old one and renames the new one into place, then recreates the old table's indexes, the triggers on it, and the triggers that write into it.

**Foreign keys on large tables are indexed.** Deleting a parent row checks every table referencing it, and an unindexed child column means a full scan per deleted row — deleting 200 images took 1.25 s scanning albums and artists, and 1 ms with `image_id` indexed. Small account tables skip the index, since scanning dozens of rows costs nothing.

---

## 8. Scanning State

**Scans and their problems are library administration, so they live in `0001_libraries.sql` beside the library they belong to.** A `scans` row mirrors the API's `Scan` object, including the resume cursor the scanner writes at every batch (`design/scanning.md` §9). Problems are two tables: `scan_problem_groups`, one row per distinct `(kind, root, normalized detail)` with a count, and `scan_problems`, one row per path, deleted when the path next scans or lists cleanly, or is gone (`design/scanning.md` §8, §13).

**Reconciliation is a column, not a set.** `tracks.last_seen_scan_id` is set on every file a scan touched, changed or not, so "what did this scan not see" is one range `SELECT` over the scope, and a scan that resumes from its cursor after a crash needs no memory of what it saw before (`design/scanning.md` §7–§8). The unseen rows are then filtered in Rust against the paths the walk could not read, and each one left is marked missing with its own `UPDATE` and feed entry.

**The analysis queue is the start of one index.** `tracks.analyzer_version` is the version of the analyzer whose results a track holds: 0 until it is analyzed, never `NULL`. The queue is then `analyzer_version < current`, and a new analyzer version needs no rewrite of any row. `tracks_unanalyzed` holds the tracks not missing in the order the queue is worked, `(library_id, analyzer_version, added_at DESC, id)`: tracks never analyzed before those an older analyzer did, and the newest first within each (`design/scanning.md` §12). A pick reads the first rows of one range and sorts nothing. At 500k tracks it takes under 0.1 ms however much is analyzed, where sorting the library by `added_at` takes 300–550 ms once half of it is, and as long for the idle poll once all of it is.

**Tracks carry no content fingerprint yet.** The scanner keys on path until track identity is settled, and the fingerprint column arrives with that design (`design/scanning.md` §6). `images.placeholder` is `NULL` until the placeholder job decodes the file into a ThumbHash, and empty when it cannot; the scanner fills the dimensions and hash from the header.

---

## 9. Not Yet Modelled

Playlists, queues, mixes, downloads, the job queue, external sources, the audit log, user settings, invites, password resets, the account change feed, and the completion-weighted score behind `sort=top`. Each arrives with its section of the API.

**Until the first release, migration files are edited in place.** There is no compatibility to keep, so a data directory made with an older development schema must be recreated. From the release on, schema changes are only ever added as new numbered migrations.

---

## 10. Open Questions

1. **Denormalization.** §3 trades scanner complexity for read speed. Computing counts on read is simpler, but costs a query per row on every browse page.
2. **Tracks with no disc tag** are stored as disc `0` and shown as `null` in the API.
