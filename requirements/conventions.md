# Shared Conventions

## Overview
Behavior that is identical across tracks, albums, artists, and playlists, defined once. Entity files state only what is genuinely specific to them and inherit everything here.

"Entity" below means any of: track, album, artist, playlist.

---

## 1. Personal Data

Per-user and private. Never shared between accounts, never visible to other non-admin users.

- **Play count** and **last played**, shown on the entity itself.
- **Listening data** — plays, listen time, and everything derived from them (`analytics.md`).

---

## 2. Actions

Every entity offers the same actions wherever it appears — in a list, a queue, a search result, or on its own page:

- Play now
- **Queue Next** and **Queue Last**
- Add to playlist
- Start radio / explore recommendations (`recommendations.md`)
- Download for offline use (`offline.md`)
- Navigate to related entities

Playing an album, artist, or playlist covers all its tracks in correct order. Shuffle is available on anything containing more than one track.

Consistency is the requirement: an entity offers the same actions everywhere, so users never learn context-specific behavior. The one exception is external content, which cannot be downloaded and is marked accordingly rather than silently offering an action that will not work (`plugins.md` §10.3).

---

## 3. Rendering

- **Everything shown is navigable.** Any entity referenced from another — a track's album, an album's artists — opens directly. Relationships are never rendered as inert text.
- **Views arrive complete.** A list shows its artwork, titles, and artists together rather than filling in progressively as a user watches.
- **Length never degrades a view.** A 5,000-track playlist opens as fast as a 10-track one.

---

## 4. Sorting & Browsing

- Sort by **sort tags where tagged** (`ARTISTSORT`, `ALBUMSORT`, `ALBUMARTISTSORT`), otherwise by display name with **leading articles stripped** — "The Beatles" files under B.
- Entities without a sortable value sort last, in a stable order that does not change between visits.
- Browsing supports sorting by name, date added, and play count at minimum, plus filtering by genre and availability.
- Sorting, filtering, and browsing stay instant at full library scale (`performance.md`).

---

## 5. Imagery

- **Artwork loads without visible delay**, at whatever size the interface needs, and never costs more bandwidth than the size being displayed warrants.
- **Artwork stays current.** Replacing an image on disk updates what users see.
- Missing imagery uses a **consistent, deliberate placeholder**. Absent art is the common case in a fresh library and must look designed, not broken.

Resolution from disk is defined in `scanning.md` §3.

---

## 6. Availability

Entities are **available** or **missing**, and clients always know which.

- Missing entities stay visible where referenced, clearly marked, and cannot be played.
- Availability on the server is distinct from **playability on this device**: a track can exist while an offline client lacks it locally. The two states are distinguishable and presented differently, because "this file is gone" and "this device doesn't have it" call for different responses.
- **Neither is an error, and both are skipped in place.** A track that is missing or not downloaded is passed over during playback without a message, leaving queue order and position untouched. This is the single rule the rest of the system refers to; nothing re-states it with different wording.

---

## 7. Multi-Artist Handling

Applies wherever an artist is displayed:

- Artists are an **ordered list**. No part of the system collapses it to a single name.
- **Every credited artist is navigable** — a featured artist is as clickable as a primary one. An artist is never rendered as inert text.
- Tagged order is preserved; the first credit is primary.

The ownership model that determines discography versus appearances is defined in `artists.md` §2.
