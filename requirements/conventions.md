# Shared Conventions

## Overview
Behavior that is identical across tracks, albums, artists, and playlists, defined once. Entity files state only what is genuinely specific to them and inherit everything here.

"Entity" below means any of: track, album, artist, playlist.

---

## 1. Personal Data

Per-user and private. Never shared between accounts, never visible to other non-admin users.

- **Play count** and **last played**, shown on the entity itself.
- **Listening data** — plays, listen time, and everything derived from them ([`analytics.md`](analytics.md)).
- **Likes**, on tracks, which are the user's Liked Songs ([`playlists.md` §11](playlists.md#11-liked-songs)), shown on each track wherever it appears.
- **Hidden tracks**, which are gone from the user's library ([§8](#8-hidden-tracks)).

---

## 2. Actions

Every entity offers the same actions wherever it appears — in a list, a queue, a search result, or on its own page:

- Play now
- **Queue Next** and **Queue Last**
- Add to playlist
- **Like** and unlike, on tracks: Liked Songs holds songs, so an album or artist is liked by liking its tracks ([`playlists.md` §11](playlists.md#11-liked-songs))
- **Hide**, on tracks ([§8](#8-hidden-tracks)); unhiding is in Hidden Songs, since a hidden track is shown nowhere else
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

- Sort by **sort tags where tagged** (`ARTISTSORT`, `ALBUMSORT`, `ALBUMARTISTSORT`), otherwise by display name with **leading articles stripped** — "The Beatles" files under B. Tags keep their leading articles — "A Cappella" files under A.
- **Accents and diacritics are ignored** — "Édith Piaf" files under E, not after "Zappa".
- Entities without a sortable value sort last in either direction, in a stable order that does not change between visits.
- Browsing supports sorting by name, date added, and play count at minimum, plus filtering by tag ([`tags.md` §7](tags.md#7-browsing-and-filtering)) and availability.
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

---

## 8. Hidden Tracks

**A user can hide a track, and a hidden track is gone from their library.** It is their one certain "no" ([`analytics.md` §7](analytics.md#7-explicit-signals)): for a track they own and never want to hear, such as a skit, an alternate take, or a song they have tired of, which nothing else would keep away.

- **Hidden means not shown.** A hidden track is in none of the user's lists: songs, albums, artists, playlists, tag pages, search results. It is never recommended or played automatically. Like all personal data (§1) it is the user's own, per library; nobody else's library changes.
- **It is never played until it is unhidden.** Playing or queueing anything that holds it, or adding it to a playlist, leaves it out, so what plays or is added is what the page showed. One already in a queue when it was hidden stays in it, unseen there as everywhere, and is skipped in place, as an unavailable track is (§6), without a message. Hiding edits no queue, so unhiding brings it back where it was.
- **What is made only of hidden tracks is hidden too.** An album, artist, or tag whose every track the user hid is gone with them, and comes back when one is unhidden, since an album with nothing to show would only point at what the user chose not to see. One with tracks left shows only those.
- **Counts are of what the user sees.** Track counts and durations, of an album, an artist, a tag, a playlist, or the library, leave out hidden tracks, so an album of twelve with one hidden is an album of eleven to that user. A count that included what they cannot see would promise tracks that are not there.
- **Hiding edits nothing.** A playlist holding a hidden track keeps it in its place, and unhiding brings it back there.
- **Hidden Songs, in the account's settings, is the one place a hidden track is shown**: every track the user hid in the library, each with Unhide. Without it a hidden track could never be found again to bring back.
- **Liking and hiding undo each other.** Liking a hidden track unhides it, and hiding a liked one unlikes it, taking it out of Liked Songs ([`playlists.md` §11](playlists.md#11-liked-songs)), so the two never contradict each other.

