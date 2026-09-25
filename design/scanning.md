# Scanning Design

## Overview
How music on disk becomes rows in the index and, later, analysis results. This file turns [`requirements/scanning.md`](../requirements/scanning.md) into a pipeline, a crate, and a set of library choices, within the budgets of [`requirements/performance.md` §5](../requirements/performance.md#5-scanning--analysis-budgets). It builds on [`general.md`](general.md); where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **incremental cost is proportional to change, and nothing that touches audio samples runs on the indexing path.**

---

## 1. Placement

```
crates/
├── core/        # TagSet, AudioProperties, Format, sort keys — pure
├── ffmpeg/      # subprocess wrapper: probe, decode, transcode, process pool
├── scanner/     # this design
└── server/      # SQLite store, admin API, wires the three together
```

- **The scanner is its own crate, not part of the core.** The core is pure by rule ([`general.md` §3](general.md#3-shared-core)); scanning is nothing but I/O. The scanner *calls* the core for normalization and sort keys, so a track's sort position is fixed at scan time and never recomputed by a client.
- **The domain types it produces live in the core.** `TagSet`, `AudioProperties`, and `Format` are what clients receive, so they are shared, and the WebAssembly build carries them at no cost.
- **Persistence is a trait the server implements.** The scanner speaks to a `Store` with a handful of batch operations (upsert tracks, mark missing, record problems, append feed entries) and never opens SQLite itself. This keeps scanner tests on an in-memory store and keeps SQLite specifics in one crate.
- **ffmpeg is a separate crate because two consumers need it.** Analysis decodes with it; playback transcodes with it ([`requirements/playback.md` §1](../requirements/playback.md#1-delivery)). One process pool with one priority policy is the only way the governor ([§10](#10-priority-and-pacing)) can see every ffmpeg process at once.

---

## 2. The Pipeline

Every file that reaches the scanner passes through the same stages, and every stage yields one `FileOutcome`: unchanged, added, updated, moved, or a problem. Stages are ordinary functions over that type; the scan loop is a fold.

| Stage | Reads | Produces | Cost per file |
|---|---|---|---|
| 1. Discover | directory entries | candidate paths + `stat` | one `readdir` per directory |
| 2. Skip | index row | unchanged, or continue | one indexed lookup |
| 3. Read | file header and tag regions | `TagSet`, `AudioProperties` | tens of kilobytes of I/O |
| 4. Identify | (deferred, [§6](#6-track-identity)) | new / moved / retagged / replaced | — |
| 5. Sidecars | directory cache | artwork, artist image, biography, lyrics refs | amortized to per-directory |
| 6. Persist | — | store batch + feed entries | amortized to per-batch |
| 7. Reconcile | index rows under scope | missing marks | one range query per scope |
| 8. Analyze | whole file, via ffmpeg | loudness, features, waveform | **never on this path** |

- **Stage 2 is the whole incremental story.** Path, size, and mtime unchanged means the file is done without being opened. Ten changed files cost ten opens; the other 499,990 cost one `stat` and one lookup each. A failure here is the failure mode [`requirements/performance.md` §5](../requirements/performance.md#5-scanning--analysis-budgets) names: a library that grows more expensive to keep current.
- **Stage 3 reads headers, never bodies.** Tags and technical properties live in the first and last kilobytes of a file. Nothing before stage 8 reads audio frames.
- **Stage 8 is a different queue on a different pool.** Indexing is what users wait for; analysis is not. Coupling them would turn a one-hour cold scan into a three-day one.

---

## 3. Formats

**Format knowledge is quarantined in two places** — `probe`, which maps a file to a `Format`, and the tag mapper ([§4](#4-tags)). Nothing downstream knows what an ID3 frame or an MP4 atom is.

- **Extension is the first filter and costs nothing.** Discovery drops every path whose extension is not in the supported set before opening it. This is what makes "unsupported files are ignored, not flagged" ([`requirements/scanning.md` §1](../requirements/scanning.md#1-supported-formats)) free: documents, images, and archives are never touched.
- **Content confirms.** For files that pass the extension filter, the probe reads the magic bytes. A `.mp3` that is really a FLAC is read as FLAC; a `.flac` that is a text file is a `corruptAudio` problem, not a crash.
- **Two libraries cover the eight formats:**

| Format | Tags and properties | Decode |
|---|---|---|
| FLAC, Ogg Vorbis, Opus | lofty | ffmpeg |
| MP3 | lofty | ffmpeg |
| AAC/M4A, ALAC | lofty | ffmpeg |
| WAV, AIFF | lofty | ffmpeg |

- **ffmpeg decodes everything.** One decoder, one code path, one set of failure modes, and Opus covered — it has no mature pure-Rust decoder. The trade is a process boundary ([§11](#11-ffmpeg)), which is a feature here, not a cost.
- **lofty is the only tag reader.** ffprobe could read tags too, but it flattens multi-value fields, hides synchronized lyrics, presents embedded pictures as video streams, and spells keys differently per container — so it would have to be followed by a normalizer that re-derives what lofty already exposes. It also costs a process spawn per file, which at the Pi's roughly thirty files per second is feasible but spends most of the cold-scan budget on `fork`. lofty is in-process, structured, and covers all eight formats, so a file lofty cannot parse is a problem, not a fallback. WMA is out of the list for exactly this reason: it would have needed a second reader ([`requirements/scanning.md` §1](../requirements/scanning.md#1-supported-formats)).

---

## 4. Tags

**One canonical `TagSet`, whatever the scheme.** This is where "tag formats differ; results must not" ([`requirements/scanning.md` §2](../requirements/scanning.md#2-tags-are-the-only-truth)) is won or lost.

lofty exposes most fields through a scheme-independent key, which does the bulk of the work. The mapper handles what survives that abstraction — the places where schemes genuinely disagree:

| Field | Where schemes diverge |
|---|---|
| Album artist | `TPE2` (ID3), `ALBUMARTIST` / `ALBUM ARTIST` (Vorbis), `aART` (MP4) |
| Compilation | `TCMP` (ID3), `COMPILATION` (Vorbis), `cpil` (MP4) |
| Track and disc totals | packed as `3/12` in ID3 and MP4; separate `TRACKTOTAL` / `TOTALTRACKS` in Vorbis |
| Release date | `TDRC` (v2.4) vs `TYER`+`TDAT` (v2.3) vs free-text `DATE`; kept at tag precision ([`requirements/tracks.md` §2](../requirements/tracks.md#2-core-metadata)) |
| Multi-value | null-separated frames in ID3v2.4, repeated fields in Vorbis and MP4, semicolons everywhere |
| Lyrics | `USLT` / `SYLT` (ID3), `LYRICS` / `UNSYNCEDLYRICS` (Vorbis), `©lyr` (MP4) |
| Pictures | APIC frames, `METADATA_BLOCK_PICTURE`, `covr` atoms; front cover preferred by type |
| Identifiers | ISRC and MusicBrainz IDs under scheme-specific names ([`requirements/tracks.md` §6](../requirements/tracks.md#6-classification--identifiers)) |

- **Multi-value handling is one function.** Native repeated fields and semicolon-separated single fields both become the same `Vec<String>`, trimmed, empties dropped, order preserved ([`requirements/artists.md` §1](../requirements/artists.md#1-identity), [`requirements/tags.md` §3](../requirements/tags.md#3-multiple-values)). The semicolon is the only delimiter; nothing splits on "feat." or "&".
- **Unknown is a value.** A missing title becomes the filename stem; missing artist and album become `None`, which the core groups deterministically. The mapper never invents a value the file did not carry.
- **ReplayGain tags are read and ignored.** Loudness is measured, not trusted ([`requirements/playback.md` §5](../requirements/playback.md#5-loudness)).
- **The golden corpus is the test.** One logical album is rendered into every supported format with identical logical metadata — multiple artists, a compilation flag, disc and track totals, a partial date, embedded art and lyrics — and a test asserts the `TagSet` for each is byte-identical after serialization. Any new format or lofty upgrade runs against it.

---

## 5. Sidecars

**Sidecar resolution is per directory, not per file.** A thousand-track folder resolves `cover.*`, `artist.*`, and `artist.txt` once, not a thousand times.

- **A directory cache lives for the duration of a scan.** Keyed by directory, it holds the resolved upward-walk result for each sidecar kind ([`requirements/scanning.md` §3.1](../requirements/scanning.md#31-album-art)–[§3.3](../requirements/scanning.md#33-artist-biographies)). Walking stops at the library root ([§3.5](../requirements/scanning.md#35-resolution-rules)); parents are resolved lazily and memoized, so a deep tree costs one lookup per distinct ancestor.
- **Lyrics are a same-directory filename match** with no walk ([§3.4](../requirements/scanning.md#34-lyrics)). The directory listing from discovery is reused; no second `readdir`.
- **Embedded wins, and is recorded as such.** A track with embedded art stores a reference to its own file's picture; one without stores a reference to the sidecar path that resolved. Both are *found* artwork; serving, resizing, and caching belong to [`requirements/conventions.md` §5](../requirements/conventions.md#5-imagery).
- **A sidecar change re-resolves the subtree.** The watcher ([§9](#9-triggers)) treats a change to a file matching a sidecar name as a scope covering its directory and all descendants, since anything below may have inherited from it ([`requirements/scanning.md` §7](../requirements/scanning.md#7-change-handling)).
- **The filenames are a public contract** that plugins write against ([`requirements/plugins.md` §3](../requirements/plugins.md#3-writing-to-the-library)). They are constants in one module and nowhere else.

---

## 6. Track Identity

**Deferred.** Content-derived identity that survives moves and retags ([`requirements/scanning.md` §6](../requirements/scanning.md#6-track-identity)) is open decision 3 in [`general.md` §11](general.md#11-open-decisions) and is not settled here.

Until it is, stage 4 keys on path: a file at a new path is a new track, and a file gone from its path is missing. This is wrong per the requirement and is understood to be temporary. What this design fixes now is the *shape* of the stage so the swap is contained:

- `identify(&FileFacts, &Store) -> Identity` takes everything stages 1–3 learned (path, size, mtime, device and inode, `TagSet`, `AudioProperties`, and the byte ranges of the tag regions, which any content fingerprint will need to skip) and returns new, moved-from, retagged, or audio-replaced.
- Nothing outside the stage reads paths to decide identity. The reconcile stage asks the store for "tracks under this scope not seen in this scan", not "tracks whose path is missing".
- The budget the eventual design must meet: whatever it reads per new file has to fit the one-hour cold scan, which rules out hashing whole files at 500,000 tracks.

---

## 7. Persistence and the Change Feed

**Batches, not rows.** The scanner accumulates outcomes and hands the store a batch of a few hundred at a time, or whatever has accumulated after a short interval, whichever comes first. One SQLite transaction per batch.

- **Unchanged files are in the batch too**, as a bare "seen by this scan" touch on the track row. That is what reconciliation ([§8](#8-reconciliation-and-missing-files)) reads instead of the scanner holding a set of every path it saw, and it is what makes resume correct: files before the cursor were already touched.
- **Each batch appends to the library's change feed** ([`requirements/libraries.md` §5](../requirements/libraries.md#5-staying-current), [`general.md` §4](general.md#4-sync-and-mutations)). That is what makes content appear progressively: a client subscribed to the feed sees the first batch seconds after a scan starts, meeting the "first tracks browsable in seconds" budget.
- **The feed carries net effect, not events.** A track added and updated within one scan is one feed entry. This is the "large import does not flood clients" requirement: a ten-thousand-track import is a few dozen feed pages, not ten thousand notifications.
- **Albums and artists are derived inside the transaction.** Album identity is title plus album artists ([`requirements/albums.md` §1](../requirements/albums.md#1-identity)); the store upserts the album and artist rows a batch implies and drops any left with no tracks, so grouping is never stale between batches.
- **Sort keys are computed by the core** before the batch is written ([`general.md` §3](general.md#3-shared-core)). The store never sorts by anything but a precomputed key.
- **Writes yield to reads.** SQLite in WAL mode lets the browse and playback paths read while a batch commits. Batch size is bounded so no single transaction holds the writer long enough to be felt.

---

## 8. Reconciliation and Missing Files

**Reconcile runs once per scope, after the walk, and only if the root answered.**

- **An unavailable root suspends its scan and touches nothing.** Before walking, the scanner `stat`s the root. Failure — not mounted, permission denied, network gone — puts the scan in the `suspended` state and leaves every row under that root exactly as it was ([`requirements/libraries.md` §2](../requirements/libraries.md#2-storage), [`requirements/scanning.md` §7](../requirements/scanning.md#7-change-handling)). The failure mode being avoided: a NAS reboot at 3 a.m. marking 100,000 tracks missing.
- **A root that answered but returned an empty listing is treated as unavailable too.** A mount point with nothing under it is far more often an unmounted share than a deleted collection. The admin can override with a manual scan of that root, which is an explicit statement that the emptiness is real.
- **Missing is a mark, not a delete.** Reconcile asks the store for tracks under the scope not seen in this scan and sets `missing_since`. Retention, playlist pinning, the 30-day purge, and immediate cleanup are store policy per [`requirements/scanning.md` §8](../requirements/scanning.md#8-missing-files), run by a scheduled job, not by the scanner.
- **A returning file clears the mark** in stage 2: a path that matches a missing row with the same size and mtime is a return, not an add.

---

## 9. Triggers

**One scanner task per library owns one coalescing queue of scopes.** A scope is a root or a folder within one. Every trigger — the filesystem watcher, the schedule, a manual request, a configuration change, a restore — does nothing but enqueue a scope. This is how "overlapping triggers never compound" ([`requirements/scanning.md` §4](../requirements/scanning.md#4-triggers)) is made structurally true rather than defended case by case.

- **A scope has a depth.** A *directory* scope visits only the files directly in a folder, which is what an ordinary file change produces; a *subtree* scope descends. Roots, manual folder scans, directory events, and sidecar changes are subtree scopes.
- **Enqueue coalesces.** A scope equal to one already queued is dropped. A scope that contains queued scopes replaces them. A scope contained by a queued or running scope is dropped. A whole-library scan therefore subsumes everything, and the API's "an equivalent scan is already running, that scan is returned" is a lookup, not a special case.
- **The watcher is `notify`, debounced.** Events are grouped by directory over a window of about two seconds and enqueued as one scope per directory, so a copied-in album is one scope, not one per file. Sidecar-named files widen the scope to the subtree ([§5](#5-sidecars)). Watcher overflow or error enqueues the whole root — the watcher is a hint, never the source of truth.
- **The schedule enqueues the whole library** on the admin's interval. It is the safety net for network mounts where `inotify` does not fire ([`requirements/deployment.md` §3](../requirements/deployment.md#3-storage--permissions)).
- **Manual scans are the API enqueuing a scope** with visible progress, which is the queue's own state ([§13](#13-progress-and-problems)).
- **Resumability is a cursor.** Discovery walks in a stable sorted order and the scan row records the last directory fully persisted. On restart, every scan in `running` resumes from its cursor. A scan that finishes with no cursor gap is complete; nothing is ever re-walked because the process died.
- **A cancelled scan stops at the next batch boundary.** Everything persisted stays persisted; the store is never rolled back to before the scan.

---

## 10. Priority and Pacing

**Scanning yields to listening** ([`requirements/scanning.md` §5](../requirements/scanning.md#5-behavior), [`general.md` §2](general.md#2-system-shape)).

- **Indexing runs on a small dedicated blocking pool** — discovery, `stat`, and tag reading are synchronous I/O and never touch the async runtime's workers. Pool size defaults to one on the Pi and a few on the desktop; storage is the bottleneck either way.
- **Analysis runs on a smaller pool still** — ffmpeg processes, one on the Pi, a few on the desktop ([§12](#12-analysis)).
- **A governor watches active streams and pauses both pools** when listening is at risk, analysis first. Pausing is `SIGSTOP` on ffmpeg children and a parked flag on the indexing pool; resuming is `SIGCONT` and unparking. A paused decode resumes where it was rather than restarting, so yielding costs nothing but time. The threshold is derived from the saturation policy in [`requirements/performance.md` §7](../requirements/performance.md#7-under-saturation).
- **Nothing here decides what a listener gets.** The governor only ever slows background work. If the host cannot serve a new stream, that refusal belongs to the playback design.

---

## 11. ffmpeg

**ffmpeg runs as a child process, never linked as a library.**

- **Isolation is the reason.** Corrupt files are routine in real collections. A decoder crash inside the server would violate "one bad file never aborts a scan" ([`requirements/scanning.md` §10](../requirements/scanning.md#10-problems)) and take listeners down with it. A child that dies is a problem record; a child that hangs is killed on a timeout; a child that must yield is stopped with a signal. None of that is available in-process.
- **Build simplicity is the second reason.** No `bindgen`, no cross-compiling libav for `arm64`, no linking questions. The container installs a pinned ffmpeg for both architectures.
- **The cost is a spawn per file and a pipe of PCM.** Negligible for analysis, which is budgeted in days, and for transcodes, which are per stream.

The `ffmpeg` crate exposes three operations behind one pool:

| Operation | Invocation (illustrative) | Used by |
|---|---|---|
| `probe(path)` | `ffprobe -v error -print_format json -show_format -show_streams` | analysis setup — channel layout for the loudness meter ([§12](#12-analysis)) |
| `decode_pcm(path)` | `ffmpeg -nostdin -v error -i <path> -map 0:a:0 -vn -f f32le -c:a pcm_f32le pipe:1` | analysis |
| `transcode(path, target)` | as the playback design specifies | playback |

- **Every invocation is read-only against the library.** No output path is ever under a root; `-nostdin` is always set; there is no `-y`. The music mount is read-only regardless ([`general.md` §2](general.md#2-system-shape)), so this is belt and braces, not the only defense.
- **Startup verifies the binary.** The server runs `ffmpeg -decoders` once, checks that every decoder the format list requires is present, and exposes the result in server info and library health ([`requirements/deployment.md` §7](../requirements/deployment.md#7-observability)). A missing decoder is one startup message, not ten thousand scan problems at 3 a.m. Missing ffmpeg entirely is fatal at startup, in the spirit of "bad configuration fails loudly" ([`requirements/deployment.md` §1](../requirements/deployment.md#1-what-the-user-provides)).
- **Exit status and stderr map to problem kinds.** A nonzero exit with a decoder error is `corruptAudio`; an unknown codec is `unsupportedEncoding`; a kill on timeout is its own group. Stderr becomes the problem's detail.

---

## 12. Analysis

**One decode, three consumers** ([`requirements/scanning.md` §1](../requirements/scanning.md#1-supported-formats), [`requirements/performance.md` §5](../requirements/performance.md#5-scanning--analysis-budgets)).

`decode_pcm` streams interleaved 32-bit float at the file's native sample rate and channel layout, which the probe supplies so the raw stream can be interpreted. The scanner reads that stream once, in blocks, and feeds every block to three sinks:

- **Loudness** — an EBU R128 meter (the `ebur128` crate) over the full channel layout, yielding integrated loudness and true peak. Measured at native rate and layout, so multi-channel and high-resolution files are measured as what they are ([`requirements/tracks.md` §4](../requirements/tracks.md#4-audio-properties)). Album loudness is aggregated by the store from the album's tracks, not by a second decode.
- **Waveform** — a mono downmix bucketed into a fixed number of peak and RMS bins per track. The shaping that makes a compressed master and an orchestral recording both legible ([`requirements/playback.md` §3](../requirements/playback.md#3-the-waveform)) is applied at render time from these bins, deterministically, so the stored data is the honest measurement and the client cannot invent structure. The bin count and shaping function are the playback design's to fix.
- **Features** — tempo, key, energy, and a compact sonic descriptor over a mono, downsampled copy of the same blocks ([`requirements/recommendations.md` §2.1](../requirements/recommendations.md#21-how-the-music-sounds)). The extractor is the recommendations design's choice; the scanner's contract is only that it consumes the same block stream and adds no second pass.

Storage and scheduling:

- **Results live in `state/`, not `cache/`.** They are rebuildable in principle, but rebuilding costs days ([`general.md` §2](general.md#2-system-shape)); losing them to a cache purge would be an outage. They are keyed by track and versioned by analyzer version, so a release that changes an algorithm reprocesses in the background ([`requirements/deployment.md` §5](../requirements/deployment.md#5-upgrades)) rather than requiring a rescan.
- **The analysis queue is a store query, not an in-memory list.** "Tracks without results at the current analyzer version, oldest first" is the queue; a restart loses nothing. A track being analyzed when the process dies is simply analyzed again.
- **Priority within the queue favors what will be heard.** Tracks in a playing or queued context and tracks recently added go first; the long tail follows. This is what makes a two-day analysis feel finished within an hour for the music people actually play.
- **A file that cannot be decoded gets an empty result at the current version**, so it is not retried on every pass. The failure is recorded as a scan problem, and a later scan that finds the file changed re-upserts the track and clears the placeholder.
- **Each track is independent.** There is no cross-track state, so a kill, a timeout, or a governor pause affects exactly one track.

---

## 13. Progress and Problems

- **Progress is the scan row.** `filesSeen`, `filesProcessed`, added, updated, moved, missing, problems, and the current path are updated on the scan row at each batch, which the admin API reads directly ([`api/openapi.yaml`](../api/openapi.yaml), `Scan`). There is no separate progress channel to keep in sync.
- **Problems are grouped by a stable key** — kind, root, and a normalized detail with paths and numbers stripped — so a permissions error on one root is one group with a count, not ten thousand rows ([`requirements/scanning.md` §10](../requirements/scanning.md#10-problems)). Individual occurrences are kept under the group for the drill-down endpoint.
- **A successful scan of a file clears its problems** in the same batch that persists it. The count on the group falls, and an empty group is removed.
- **A scan that had problems says so.** The scan row records the problem count; the library health view shows outstanding groups. A partial result is never reported as a clean scan.

---

## 14. Duplicates

**A background job over the index, never part of a scan** ([`requirements/scanning.md` §9](../requirements/scanning.md#9-duplicates)).

- **Matching tags** — same title, artists, album, and track number; a store query.
- **One album under two paths** — same album identity spanning two directories; a store query.
- **Matching content** — awaits track identity ([§6](#6-track-identity)); whatever fingerprint identity adopts, duplicates by content is an equality query on it.

The job runs on the indexing pool at the lowest priority, writes only to a duplicates table, and never touches a track row. The report is the API's `DuplicateGroup`.

---

## 15. Verification

- **Golden corpus** ([§4](#4-tags)) — identical logical metadata in every format yields identical `TagSet`s; run on every build and on every lofty or ffmpeg upgrade.
- **Generated library** — the `bench/` generator emits a library at full scale for each verification machine with realistic tag variety, sidecars at every level, and a fraction of deliberately corrupt files. Cold scan, ten-file incremental, and copied-in-album latency are measured against [`requirements/performance.md` §5](../requirements/performance.md#5-scanning--analysis-budgets) at p95 and p99.
- **Fault injection** — a root unmounted mid-scan leaves the index intact and the scan suspended; a hung ffmpeg is killed within its timeout and recorded; a kill of the server mid-scan resumes from the cursor with no duplicate rows and no missed files.
- **Isolation** — a scan of one library never reads a path under another library's root and never writes a row outside its library ([`requirements/general.md` §3.6](../requirements/general.md#36-libraries-are-the-isolation-boundary)).
- **Read-only** — the whole suite runs against a read-only mount and passes.

---

## 16. Open Decisions

1. **Track identity** ([§6](#6-track-identity)) — open decision 3 in [`general.md` §11](general.md#11-open-decisions). This design fixes the stage's interface and budget, not its algorithm.
2. **Waveform bin count and shaping function** — the playback design's, constrained here to be derived from stored peak and RMS bins with no second decode.
3. **Feature extractor** — the recommendations design's, constrained here to consume the shared block stream.
4. **Analysis pool sizes and the governor threshold** — defaults above are starting points; the benchmarks on both verification machines decide the shipped values.
