# Playlist Requirements

## Overview
A playlist is an ordered list of tracks. Playlists come in two kinds, and both are called playlists throughout the interface — users should never have to learn a second word for one of them.

| | Where it appears | How long it lasts |
|---|---|---|
| **Saved playlist** | Among the user's playlists | Until they delete it |
| **Generated playlist** | Among recommendations ([`recommendations.md`](recommendations.md)) | Temporary, unless the user saves it |

A **filtered view** — the library narrowed by genre, year, or listening data and played directly — is not a playlist. It is a way of browsing, and it leaves nothing behind. Users filter constantly, and doing so must never accumulate hundreds of accidental playlists.

The requirements below describe **saved** playlists unless stated otherwise. Generated playlists are specified in [`recommendations.md`](recommendations.md); [§8](#8-saving-a-generated-playlist) covers what happens when one is saved.

Shared behavior follows [`conventions.md`](conventions.md).

---

## 1. Ownership & Scope

- **Playlists are strictly personal.** Every playlist belongs to one user. There is no sharing, no collaboration, and no visibility to other non-admin accounts ([`users.md` §7](users.md#7-privacy--personal-data)).
- **A playlist belongs to one library** and holds only tracks from it ([`libraries.md` §4](libraries.md#4-isolation)).
- Losing access to a library **preserves** its playlists; they return intact if access is restored.

---

## 2. Metadata

- **Title**, required.
- **Description**, optional and free-form.
- **Artwork** ([§6](#6-artwork)).
- **Track count** and **total duration**, shown wherever the playlist appears.
- **Created** and **last modified** times.
- **Play count** and **last played**, per [`conventions.md` §1](conventions.md#1-personal-data).

---

## 3. Contents

- **Order is the user's.** A playlist plays in the order it was arranged and never silently reorders itself.
- **Duplicates are allowed, with a warning.** Adding a track already in the playlist works, but the user is told it is already there and can back out. Intentional repeats are legitimate; accidental ones are the common case.
- **Missing tracks stay put.** A track whose file has vanished remains in position, clearly marked, skipped on playback ([`conventions.md` §6](conventions.md#6-availability)), and retained for as long as any playlist references it ([`scanning.md` §8](scanning.md#8-missing-files)). If the file returns, it plays again.
- **There is no arbitrary size limit.** A playlist of tens of thousands of tracks is supported, and the host is the only limit ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)).

---

## 4. Editing

Every edit is immediate in the interface and reconciled afterward ([`general.md` §3.4](general.md#34-optimistic-by-default)).

- **Add and remove in bulk.** Selecting many tracks and adding or removing them is one action, and it either fully succeeds or fully fails — never half-applied.
- **Reorder by dragging**, one track or many.
- **Move non-contiguous selections together.** A user can select tracks scattered through a playlist — the 2nd, the 7th, the 12th — and move all of them to one new position in a single action, preserving their relative order. Selections do not have to be adjacent.
- **Add from anywhere.** Any track, album, artist, or playlist encountered anywhere in the app can be added to a playlist, including to a new one created in the moment.
- **Totals stay correct instantly.** After any edit, track count, duration, and artwork reflect the change immediately, with no visible refresh.
- **Removals are recoverable.** Removing tracks, or deleting a playlist, can be undone — deleting a hundred-track playlist by mistake must not be final.

---

## 5. Concurrent Edits

A user editing from several devices must never silently lose work or find changes they did not make.

- **Edits apply only to the version the user was actually looking at.** If the playlist changed since, the edit is refused rather than applied on top of something unexpected.
- **This does not make rapid editing fragile.** A device's own edits advance the version it holds, so a user reordering ten tracks in a row is never refused by their own previous edit. Refusal means a *different* device or session changed the playlist — the case [§5](#5-concurrent-edits) exists for.
- **A refused edit is explained.** The user is told plainly that the playlist changed elsewhere and their change was not applied — never left believing it worked.
- **Nothing is merged or overwritten silently.** The playlist is never quietly reshaped by combining two versions, and a later edit never erases an earlier one without the user knowing.
- **Offline edits follow the same rule.** A device that edited while offline syncs successfully if nothing changed meanwhile, and reports the failure clearly if something did.
- **Edits reach other devices immediately** ([`realtime.md`](realtime.md)). A change on a phone appears on a desktop without refreshing.

---

## 6. Artwork

- **Generated by default** — a mosaic built from the album art of the tracks inside, updating as the contents change.
- **Custom artwork is optional** and overrides the generated image. It lives in Jewelcase's own storage; the music library is never written to ([`general.md` §3.1](general.md#31-the-music-library-is-read-only-to-core)).
- An empty playlist shows a **deliberate placeholder**, not a broken image.

---

## 7. Organization

- **Sort** by name, date created, date modified, and play count.
- **Filter and search** within the playlist list by name.
- **Pin** playlists to keep frequently used ones at the top.
- **Search within a playlist** for a track, without leaving it.

---

## 8. Saving a Generated Playlist

Generated playlists live among recommendations and are temporary. Saving one moves it into the user's own playlists, where it:

- Becomes a **saved playlist** they own and can edit like any other, indistinguishable from one built by hand.
- **Keeps the tracks as they were** at the moment of saving. It does not keep regenerating, and it does not change underneath the user afterward.
- Arrives with a **sensible default title** they can immediately change.

Until saved, a generated playlist must be **stable enough to come back to** — a user can play it, navigate away, and return to find the same playlist rather than a freshly generated one.

---

## 9. Export

- **Standard playlist files** (`.m3u`), so a playlist works in any other music player.
- **A complete format** preserving description, artwork, order, and timestamps, for backup and for moving between Jewelcase servers.

Note the deliberate asymmetry: Jewelcase **writes** playlist files on export but **never reads** them from a library. Playlist files found while scanning are ignored, so every playlist has exactly one owner and one authoritative version.

---

## 10. Playlist-Specific Behavior

Beyond [`conventions.md`](conventions.md):

- **Play and queue actions use the playlist's current order**, not a stored snapshot of it.
- **Download is a standing instruction**, covering tracks added later while the download remains active ([`offline.md` §3](offline.md#3-what-can-be-downloaded)).
