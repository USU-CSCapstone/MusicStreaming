# Scanning Requirements

## Overview
Scanning is how music on disk becomes music in Jewelcase. It is the only ingestion path — no import wizard, no manual entry, and no way for any component to add content except by placing it in a library and letting the scanner find it. This holds whether a file arrived by a user copying it over a network share or by a plugin writing it.

That single path is what keeps the architecture simple: content is content, regardless of who produced it.

The scanner reads and never writes. When something is ambiguous or unavailable it preserves what it already knows rather than assuming the user destroyed something.

---

## 1. Supported Formats

**FLAC, ALAC, WAV, AIFF, MP3, AAC/M4A, Ogg Vorbis, Opus, and WMA.**

- **Unsupported files are ignored, not flagged.** A library containing documents, images, or archives scans cleanly without noise about files Jewelcase was never meant to index.
- **Supported-but-unreadable files are reported** as scan problems rather than silently dropped.
- **Technical properties are captured accurately** — duration, sample rate, bit depth, channel count, bitrate, codec — including multi-channel and high-resolution audio. Playback and transcoding depend on these being right.
- **Every track is analyzed**, in **a single decoding pass** producing three results:
  - its **loudness**, so normalization is consistent however the files were tagged ([`playback.md` §5](playback.md#5-loudness));
  - a description of **how it sounds** — tempo, key, energy, sonic character — which is what lets recommendations reach music never played and never tagged ([`recommendations.md` §2.1](recommendations.md#21-how-the-music-sounds));
  - its **waveform**, which the client draws as the progress bar so a listener can see the shape of a track and scrub to a point in it ([`playback.md` §3](playback.md#3-the-waveform)).

  **All three come out of one decode.** Decoding is nearly the entire cost of analysis, so any result that needs the audio is derived in that pass or not at all — adding a second pass would multiply the most expensive work in the system ([`performance.md` §5](performance.md#5-scanning--analysis-budgets)).
- **Analysis is background work and never blocks a library.** Results are stored in Jewelcase's own data, never written to the library, and tracks not yet analyzed are simply less well served rather than unavailable. Its cost and priority are budgeted in [`performance.md` §5](performance.md#5-scanning--analysis-budgets).

---

## 2. Tags Are the Only Truth

**Embedded tags are the sole source of track metadata. Folder structure is never interpreted as meaning.**

A file at `/music/Nirvana/Nevermind/01 - Smells Like Teen Spirit.flac` tells Jewelcase nothing about the artist, album, or track number. Only its tags do. The system therefore behaves identically for every collection regardless of arrangement, and there is exactly one place to look when metadata is wrong.

- **Albums and artists are formed entirely from tag values.** Two files in different folders with matching tags are the same album. Two files in one folder with different album tags are not.
- **Untagged files are indexed with what they have** — a track named after its filename, attributed to unknown artist and album. Playable, searchable, queueable, never discarded.
- **Unknown is a real value, not an error.** Tracks with no artist or album group together predictably rather than scattering.
- **Multi-value tags are preserved**, not flattened into a single string.
- **Disc and track numbers come from tags**, including totals where present.
- **Compilations and various-artist albums** are recognized from album-artist and compilation tags, so a compilation stays one album rather than fragmenting per contributing artist.
- **Tag formats differ; results must not.** The same logical metadata in different tagging schemes produces the same result.
- **Jewelcase does not edit tags and offers no interface to do so.** Metadata is corrected by correcting the files — by the user, or by a plugin. An in-application override would create a second, invisible source of truth diverging from disk.

---

## 3. Sidecar Content

Artwork, artist imagery, biographies, and lyrics cannot live in track tags, so Jewelcase finds them by convention.

**These rules are a public contract.** Plugins deliver results by following them exactly, which is what makes plugin-produced content indistinguishable from content placed by hand. Changing them breaks every plugin, so they are treated as a stable interface.

### 3.1 Album Art
1. **Embedded artwork wins.**
2. Otherwise, a file named **`cover.*`** (any supported image extension) in the track's directory.
3. Otherwise, move up to the parent directory and look again, repeating until found or the library root is reached.

### 3.2 Artist Images
A file named **`artist.*`**, resolved by the same upward walk.

### 3.3 Artist Biographies
A file named **`artist.txt`**, resolved by the same upward walk.

### 3.4 Lyrics
1. **Embedded lyrics win.**
2. Otherwise, matched by **filename** — the audio file's base name with a lyrics extension. `01 - Smells Like Teen Spirit.flac` pairs with `01 - Smells Like Teen Spirit.lrc`.

Lyrics resolve **only in the track's own directory**. There is no upward walk — inheriting them from a parent folder would attach the wrong words to the wrong song.

Both time-synchronized and plain-text lyrics are read ([`tracks.md` §5](tracks.md#5-lyrics)).

### 3.5 Resolution Rules
- **The nearest match wins.** A per-album cover overrides a per-artist one, which overrides one at the library root.
- **The walk never escapes the library**, stopping at the root folder.
- **A missing sidecar is normal**, producing a clean empty state, never a scan problem.
- **Sidecar changes are picked up like any other change**, with no admin action.
- **Sidecars are read, never written.** Only plugins write to a library.

---

## 4. Triggers

- **Live filesystem watching.** Changes detected as they happen, so a copied-in album or a plugin-written biography appears within seconds.
- **Scheduled scans.** Periodic reconciliation on an admin-configured interval — the safety net for network mounts, where live watching is frequently unreliable.
- **Manual scans.** On demand, scoped to a library or a single folder, with visible progress.

**Overlapping triggers never compound.** Several triggers firing at once cost no more than one, and never leave a library scanning itself repeatedly.

---

## 5. Behavior

- **Scanning is always a background operation.** The library stays browsable, searchable, and playable throughout — including the very first scan of a new library. There is no state in which Jewelcase is unusable because it is indexing.
- **Content appears progressively**, as found rather than all at once on completion.
- **Incremental work is proportional to change.** Ten new files cost what ten files cost, never what the library costs.
- **Scans are interruptible and resumable**, surviving restarts without starting over.
- **Scanning yields to listening.** Under contention, serving listeners wins.
- **Progress is visible** — what is being scanned, how far along, what it found, what it had trouble with.

---

## 6. Track Identity

**A track's identity follows its content, not its path.** Moving, renaming, or reorganizing files is recognized as the same tracks in new locations, with playlists, history, and statistics intact.

This is a hard requirement, not a best effort. Reorganizing a music collection is a normal thing to do, and a system that punishes it by destroying playlists is broken.

**Identity is independent of tags too.** Neither retagging nor moving creates a new track, in any combination.

---

## 7. Change Handling

- **Tags changed.** Updates in place, keeping identity, playlist memberships, and history. Grouping changes are applied faithfully — retagging an album moves those tracks, and albums or artists left with no tracks disappear.
- **Audio replaced.** Re-read, technical properties updated.
- **File moved or renamed.** Same track, new location ([§6](#6-track-identity)).
- **Sidecar added, replaced, or removed.** Re-resolved per [§3](#3-sidecar-content), affecting every track that inherited from it.
- **Root or exclude rules changed.** Newly included content indexed; newly excluded content treated as missing.
- **Storage unavailable.** Scanning suspends for that root; the index is left intact. An unmounted drive is never interpreted as deleted music.

---

## 8. Missing Files

A track no longer on disk is **marked missing, not removed** — visible, clearly unavailable, unplayable.

- **Retained indefinitely if referenced by any playlist.** If the file returns, the track reconnects with nothing lost.
- **Removed after 30 days** otherwise. Playlist membership is the only reference that extends retention — a kept queue ([`queue.md` §6](queue.md#6-sessions)) expires on its own schedule and does not pin a track, and a copy downloaded to a device does not either ([`offline.md` §9](offline.md#9-download-lifecycle)).
- **Admins can clean up immediately**, without waiting out the retention period.

Retention is deliberately generous: an unmounted NAS must not quietly dismantle everyone's playlists, and a stale record costs far less than a destroyed one.

### 8.1 Purging Never Costs History
Removing a track record must leave listening history readable and statistics unchanged. History carries its own snapshot for exactly this reason ([`analytics.md` §8](analytics.md#8-durability)).

---

## 9. Duplicates

**Every file is its own track.** A FLAC and an MP3 of one song are two tracks, both visible and playable.

- Jewelcase **detects likely duplicates** — matching tags, matching content, one album under two paths — and reports them to admins.
- The report is **informational only.** Jewelcase never deletes, merges, or hides anything on its own; which copy to keep is the owner's judgment.
- Detection runs in the background and never delays scanning or playback.

---

## 10. Problems

A scan never fails as a whole.

- **One bad file never aborts a scan.** Unreadable files, permission failures, and malformed tags are collected; the scan continues.
- **Problems are reviewable**, grouped so a systemic issue reads as one problem rather than ten thousand.
- **Problems clear themselves** when a file later scans successfully.
- **Problems are honest.** A library with unreadable content says so rather than reporting a clean scan over a partial result.
