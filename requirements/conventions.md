# Shared Conventions

## Overview
Behavior that is identical across tracks, albums, artists, and playlists, defined once. Entity files state only what is genuinely specific to them and inherit everything here.

"Entity" below means any of: track, album, artist, playlist.

---

## 1. Personal Data

Per-user and private. Never shared between accounts, never visible to other non-admin users.

- **Play count** and **last played**, shown on the entity itself.
- **Listening data** — plays, listen time, and everything derived from them ([`analytics.md`](analytics.md)).

---

## 2. Actions

Every entity offers the same actions wherever it appears — in a list, a queue, a search result, or on its own page:

- Play now
- **Queue Next** and **Queue Last**
- Add to playlist
- Start radio / explore recommendations ([`recommendations.md`](recommendations.md))
- Download for offline use ([`offline.md`](offline.md))
- Navigate to related entities

Playing an album, artist, or playlist covers all its tracks in correct order. Shuffle is available on anything containing more than one track.

Consistency is the requirement: an entity offers the same actions everywhere, so users never learn context-specific behavior. The one exception is external content, which cannot be downloaded and is marked accordingly rather than silently offering an action that will not work ([`plugins.md` §10.3](plugins.md#103-its-limits)).

---

## 3. Rendering

- **Everything shown is navigable.** Any entity referenced from another — a track's album, an album's artists — opens directly. Relationships are never rendered as inert text.
- **Views arrive complete.** A list shows its artwork, titles, and artists together rather than filling in progressively as a user watches.
- **Length never degrades a view.** A 5,000-track playlist opens as fast as a 10-track one.

---

## 4. Sorting & Browsing

- Sort by **sort tags where tagged** (`ARTISTSORT`, `ALBUMSORT`, `ALBUMARTISTSORT`), otherwise by display name with **leading articles stripped** — "The Beatles" files under B.
- **Accents and diacritics are ignored** — "Édith Piaf" files under E, not after "Zappa".
- Entities without a sortable value sort last in either direction, in a stable order that does not change between visits.
- Browsing supports sorting by name, date added, and play count at minimum, plus filtering by genre and availability.
- Sorting, filtering, and browsing stay instant at full library scale ([`performance.md`](performance.md)).

### 4.1 Track Lists

- **Every list of tracks sorts the same way** — the library, an album, an artist, a playlist, search results — by **title, artist, album, release year, and duration**. Playlists add **date added** ([`playlists.md` §10](playlists.md#10-playlist-specific-behavior)). One set of sorts everywhere, so it is learned once (§2).
- **The sorts live in a header that stays in view as the list scrolls.** Long lists are where sorting matters, and a header that scrolls away sends the user back to the top to change it. Where the columns do not fit, the same sorts are one control away, never absent ([`general.md` §4](general.md#4-target-platforms)).
- **One control, three states: ascending, descending, then unsorted.** Unsorted is the list's natural order — a playlist's arrangement, an album's discs and track numbers ([`albums.md` §4](albums.md#4-track-listing--completeness)), search's relevance, the library's most recently added. It is always one step away, because that order carries meaning a sort hides.
- **Sorting is a view, never an edit.** Sorting a playlist leaves its order exactly as arranged ([`playlists.md` §3](playlists.md#3-contents)). A sort that quietly rewrites a playlist destroys work the user cannot get back.
- **A sorted list plays in the order shown.** What the user sees next is what plays next, from a row or from the list's Play action.
- **A list already on the device sorts there, at once, in exactly the order the server would give it** ([`general.md` §3.5](general.md#35-offline-parity)). Waiting on the network to reorder rows already shown would make the commonest sort the slowest, and a client that orders by its own locale would disagree with itself across devices.

---

## 5. Imagery

- **Artwork loads without visible delay**, at whatever size the interface needs, and never costs more bandwidth than the size being displayed warrants.
- **Artwork stays current.** Replacing an image on disk updates what users see.
- Missing imagery uses a **consistent, deliberate placeholder**. Absent art is the common case in a fresh library and must look designed, not broken.

Resolution from disk is defined in [`scanning.md` §3](scanning.md#3-sidecar-content).

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

The ownership model that determines discography versus appearances is defined in [`artists.md` §2](artists.md#2-ownership-discography-vs-appearances).
