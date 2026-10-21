# Database Design

## Overview
The SQLite schema behind `api/openapi.yaml`. Migrations are plain SQL in `crates/server/migrations/`, applied in order at startup (`design/general.md` §2). There is one file per domain, so each table's whole definition is in one place: libraries and scanning, accounts, the catalog, the change feeds, listening and searching activity, and plugins. Sections of the API not yet built add their tables as they are (§9).

---

## 1. Conventions

| | Rule |
|---|---|
| **IDs** | Random positive 63-bit integers as `INTEGER PRIMARY KEY`, generated in each `INSERT` by `random() & 0x7FFFFFFFFFFFFFFF`. A collision, about one in 10¹³ per insert at a million rows, fails its transaction rather than being retried. The API shows them as decimal strings, which clients treat as opaque. |
| **Timestamps** | Integer milliseconds since the Unix epoch, UTC, named `*_at`. |
| **Durations** | Integer microseconds, named `*_us`, matching the API. Playback positions and listen time are the exception: integer milliseconds named `*_ms` (`player_states.position_ms`, `plays.listen_time_ms`), because the API carries them in milliseconds (`positionMs`, `listenTimeMs`). A track's `duration_us` holds whole milliseconds, the precision lofty, the tag reader, reports for every format. |
| **Booleans** | `INTEGER` with `CHECK (x IN (0, 1))`. |
| **Indexes** | Partial and unique indexes, feeds (ending in `seq`, named `_feed`) and sweeps (`_age`) are named for their purpose: `tracks_missing`, `tracks_file`, `library_changes_feed`, `search_events_age`. Every other index is `<table>_by_<key>`, leaving out a leading `library_id`: `tracks_by_added`, `plays_by_user`. |
| **Tables** | `STRICT`, so a column holds only its declared type. Link tables are `WITHOUT ROWID`. |
| **Names** | A closed set of names is `CHECK`ed, such as `scans.trigger`. Plugin permission and hook names are not: they grow with the plugin contract, and Rust checks them (`design/plugins.md` §6). |
| **JSON** | Only for values never queried by field: settings, exclude patterns, labels, pass-through identifiers, a play's artists and genres. A query reaches rows only through indexes on ordinary columns, and may then expand an array of plain values with `json_each`; nothing in a JSON value is indexed or used to find the row that holds it, and no query looks inside an object. Always `CHECK`ed as valid and of the right shape. |
| **Secrets** | Tokens and codes are stored as SHA-256 hashes, passwords as PHC strings. Nothing secret is stored readably. |

**Why random IDs.** Sequential IDs are shared across libraries, so a user who sees track 1000 and then track 1003 learns that tracks 1001 and 1002 exist somewhere they cannot reach. That breaks "inaccessible is indistinguishable from nonexistent" (`requirements/users.md` §10). Random 64-bit IDs stay as compact as sequential ones for the catalog on device, and reveal nothing. Internal-only tables, such as `login_attempts` and the feeds, keep ordinary sequential rowids.

**An ID a client chooses is keyed with its user.** A play keeps the `playId` its device chose, so it is counted once however often it is reported (`requirements/analytics.md` §6), and any other user can choose the same one, by chance or on purpose. So `plays` is keyed by `(user_id, id)`, `play_events` refers to a play by both, and one user's report never reaches another's play.

**A signed-out device keeps its token hash.** `devices.revoked_reason` marks it signed out, so a request with its token is answered `401 session_revoked` with the reason, which tells the device whether to keep its downloads (`requirements/offline.md` §9). Its row is deleted only by the device's own `logout` or with its account. Every sign-in makes a new random token, so a revoked one never authenticates again, and a suspended account's tokens, signed out or not, answer as unknown ones do (`requirements/users.md` §10).

---

## 2. Isolation by Construction

**Every content table carries `library_id`, and every link between content rows is a composite foreign key on `(library_id, id)`.** A track can only reference an album, root, or artist in its own library, and a link across libraries fails at insert (`requirements/general.md` §3.6). Deleting a library cascades through everything it holds, and removes nothing outside it. A user's plays are theirs rather than the library's, so they stay, with `library_id` cleared (§6).

**`plugin_grants` and `plugin_settings` keep what applies to a plugin everywhere as library 0**, which no library can have, since `libraries.id` is `CHECK`ed above zero. So neither can reference `libraries`, and a trigger on `libraries` deletes a library's rows in both with it.

**A composite foreign key cannot use `ON DELETE SET NULL`**, because it would clear `library_id` too, which is `NOT NULL`. So the links to an image, an album, or a root (`artists.image_id`, `albums.image_id`, `tracks.album_id`, `tracks.root_id`, `images.root_id`) take no action on delete, and such a row cannot be deleted while anything still refers to it. Deletes run in dependency order: an album goes only once no track is left on it, and anything that deletes images must delete only those nothing refers to (nothing deletes images yet).

---

## 3. Derived Data the Scanner Maintains

The scanner is the only writer of content (`requirements/general.md` §3.2), so derived values are kept current in the same transaction as the change that affects them. Reads, which dominate, never recompute them.

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

The displayed name — `artists.name`, `albums.title`, `tags.name` — is the spelling most files use, so it follows the collection. Each track records how its file writes every name: `track_artists.artist_name`, `track_album_artists.artist_name`, `tracks.album_title`, and `track_tags.tag_name`. Names come from these without rereading files, and ties break by the spelling's bytes so a tie never flips the name between scans.

**Counts follow each change by ±1.** Triggers on tracks and their links (`0007_catalog_counts.sql`) keep every count that one change moves by one: the library's tracks, albums, artists and duration; an artist's tracks, available tracks (which follow `missing_since`), albums and appearances (which follow the album's artists); a tag's tracks, albums and artists. `album_tags` and `artist_tags` count the tracks that bring each genre, so a row goes with the last of them. Spellings are counted the same way, in `artist_spellings`, `tag_spellings`, and `artist_sort_keys` (the sort keys of the tracks crediting the artist first), and the scanner reads the top of each for the artists and tags a batch touched, since only the shared core can make a sort key from a name. So a batch's derived work is proportional to the rows it changes, however many tracks the artists and tags it touches hold. A track's artist credits and genres are deleted before the track, since the counts they feed read it, and an artist or tag is deleted when no credit or genre names it, never because a count reached zero.

**Albums are resolved from their tracks.** A disc or track total is a maximum, the release date and title are the most-used values, and the album artists are its lowest-id track's, none of which ±1 can keep. So the scanner recomputes each album a batch touched from that album's tracks, which costs the album's size, and leaves album artists already in place alone so their counts don't churn.

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

**A play keeps a snapshot of its track**, so its history and statistics outlive the track (`requirements/analytics.md` §8, `requirements/scanning.md` §8.1). `plays` holds the track's title, duration and release date, and its artists, album and genres as the catalog names and matches them (§3): `artists` and `artist_keys` are the artists' names and `name_key`s in credit order, `album`, `album_title_key` and `album_artists_key` are the album's title and keys, and `genre_keys` are the genres' `name_key`s. The unknown artist and album are kept too, since untagged music is listened to as well: the unknown artist is a `null` name with the key `''`, and the unknown album a `NULL` title with an empty title key and its artists' key, as the catalog keys it. The same credit query, with no filter, feeds both artist arrays (`credits_json!`), and a `CHECK` holds their lengths equal, so they line up by position. The `INSERT … SELECT` that records a report reads all of it from the track it already checks.

**A snapshot keeps keys, not row IDs.** The catalog deletes an artist or album with its last track and makes a new row, under a new random ID, when its music comes back, so an ID would split one artist's plays in two. The keys are the catalog's identity: they survive a purge and a return, and the unknown artist and album have them. A statistic groups by `(library_id, key)`, and, for a play in a library the user reaches, finds the live artist, album or genre through the unique indexes on `(library_id, name_key)`, `(library_id, title_key, artists_key)` and `(library_id, type, name_key)`. A per-entity statistic expands the arrays of the user's plays it scans anyway, so it needs no link table. The cost: a change to how the core folds names must also re-fold the keys in `plays`, in the same migration that re-folds the catalog's. It can re-fold artist and album-title keys from the stored names, and genre and album-artist keys from the keys themselves, which works only when the new folding folds more.

**A later report refreshes the snapshot**, which improves "last known" only for the few plays reported again after a retag, but costs nothing, since the row is rewritten anyway. A report that names another track than the play's changes nothing of it. Once the track is purged no report finds it, so the play keeps what it last had. `plays.track_id` has no foreign key, so a purge never deletes a play, and random IDs (§1) make it vanishingly unlikely that a purged track's ID is ever another track's.

**History shows a user's own plays wherever they were heard.** It reads the live track through `LEFT JOIN tracks ON tracks.id = p.track_id AND tracks.library_id = p.library_id`, only in libraries the user reaches; any other play, purged or out of reach, shows its snapshot, marked unavailable, with `trackId: null`. History leaves the unknown artist's `null` out of the API's `snapshot.artists`, so it stays a list of names. Deleting a library clears its plays' `library_id` (`ON DELETE SET NULL`), since `libraries.id` can be used again and a new library must not inherit them; its `play_events` go with it, so no hook delivers them. Such a play is never found by a report again, and appears only in views across all libraries, as does a play in a library the user can no longer reach (`requirements/analytics.md` §8). Every read treats a play whose library the user does not reach exactly as a library-less one: no live join, `libraryId` null, the same `availability` as a purged track, and grouped under `(NULL, key)`. Otherwise a revoked library would show through: history would name it where a deleted one is null, and a statistic would join its live entities (`requirements/users.md` §10). Every read is by `user_id`, which keys `plays` (§1), so no other user ever sees them. An admin viewing an account's history or statistics (`requirements/analytics.md` §9) sees exactly what that user sees, these plays included, with reach judged as that user's.

**Hooks that act for one user read their domain's outbox** (`design/general.md` §4), written by the same transaction as the activity:

- `play_events` holds each play's start and end, for `playing` and `played`, with a `kind` and one sequence. The feed index `play_events_feed` is keyed `(library_id, user_id, kind, seq)`, so each hook's due check and batch read one range of its own kind, as `library_changes_kind_feed` does for the change hooks. A position caught up on ends never steps over later starts, nor the reverse, though ends pile up when a device catches up after being offline. Stale starts are swept on each new play through `play_events_started_age`, a partial index on starts alone, so the sweep never reads an end.
- `search_events` names each search or forget, and keeps what a search found. Rows are written only for a user who shares their searches, and only then is the search index asked to count, so no one else's search pays for it.

**A user's connection to a plugin is one row of `plugin_connections`**, keyed by plugin and user and the same in every library: their personal settings, when they connected, and `searches_shared_at`, null until they share their searches with it. Sharing is a column of the connection rather than a table of its own, so it can never exist without one, and disconnecting deletes both. Settings are kept as entered, secrets included, because the plugin needs them; the API never returns the secrets (`design/plugins.md` §7). A connection goes with its user, and with the plugin: unlike grants and settings, which are keyed by the plugin's ID and outlive an uninstall, it references `plugins`, so its users connect again after a reinstall (`0006_plugins.sql`, `design/plugins.md` §6).

---

## 7. State and Cache

| In the database (`data/state/`) | Rebuildable (`data/cache/`) |
|---|---|
| Everything above | Resized images |
| Waveforms and loudness — analysis takes days to redo | Search indexes |
| | Transcodes |

**Every connection must enable `PRAGMA foreign_keys = ON`.** SQLite leaves foreign keys off by default, and §2 depends on them.

Migrations are the exception: they run with foreign keys off, because rebuilding a parent table with them on deletes or clears every reference to it. Each migration commits only if `PRAGMA foreign_key_check` finds no broken reference anywhere in the database. A rebuild creates the new table, copies the rows, drops the old one and renames the new one into place, then recreates the old table's indexes, the triggers on it, and the triggers that write into it.

**Foreign keys on large tables are indexed.** Deleting a parent row checks every table referencing it, and an unindexed child column means a full scan per deleted row — deleting 200 images took 1.25 s scanning albums and artists, and 1 ms with `image_id` indexed. Small account tables skip the index, since scanning dozens of rows costs nothing. `plays.library_id` is not indexed: its parent goes only with a whole library, whose catalog cascade dwarfs one scan of `plays`.

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

1. **Tracks with no disc tag** are stored as disc `0` and shown as `null` in the API.
