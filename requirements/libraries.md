# Library Requirements

## Overview
A library is a collection of music on disk and everything Jewelcase knows about it. Libraries are the isolation boundary for the entire system: every track, album, artist, playlist, queue, and play record belongs to exactly one.

Jewelcase's relationship with a library is one-directional. It **reads**. It does not organize, rename, retag, or place anything beside the files. A collection must survive Jewelcase being installed, run for years, and uninstalled without a single byte having changed.

This file covers what a library is, where it lives, and how it is administered. How its contents are read off disk is covered in [`scanning.md`](scanning.md).

---

## 1. What a Library Is

- Defined by **one or more root folders**, whose combined contents form the library. A collection spanning two drives is one library.
- Has a **name**, set by the admin, shown to users when choosing where to browse.
- **Exclude rules** filter what gets indexed — sample packs, backups, working directories — during scanning, so excluded content costs nothing.
- **Roots must not overlap**, within or across libraries. A file belongs to exactly one library; a configuration that would make membership ambiguous is rejected rather than resolved silently.
- Changing roots or exclude rules triggers reconciliation.

---

## 2. Storage

- **Local disks and network mounts are both first-class.** NFS, SMB, and similar must work as well as local disk, since a homelab collection commonly lives on a NAS.
- **Jewelcase never requires write permission to a music library.** A library on read-only storage works completely — scanning, playback, artwork, lyrics — because the core has no reason to write. Read-only storage costs a library its plugin-supplied enrichment and nothing else.
- **Storage that disappears is not data loss.** An unmounted drive or unreachable share leaves the index intact and is never interpreted as deleted music.
- **Path changes are survivable.** A root that moves can be repointed without re-indexing from scratch and without breaking playlists, history, or statistics.

---

## 3. Management

- Admins can **create, rename, reconfigure, and delete** libraries at any time.
- **Deleting a library removes only Jewelcase's knowledge of it** — the index, and the playlists, queues, history, and statistics scoped to it. It never deletes, moves, or alters a file on disk. The confirmation must say so unmistakably, since it is the one place a user might reasonably fear otherwise.
- **Plugins are enabled per library**, toggled independently for each.
- **Library access is granted per account** ([`users.md`](users.md)).
- Admins see per-library health at a glance: track count, total size, last scan time, current scan state, unavailable roots, missing tracks, outstanding scan problems.

---

## 4. Isolation

- Every entity carries its library, and every request is authorized against it.
- **Search, browse, recommendations, and queue generation never cross a library boundary** — including for admins, who hold access to every library but operate within one at a time.
- **Playlists and queues cannot mix libraries.** A playlist belongs to a library, and every track in it comes from that library.
- **Access, not identifiers, decides reach.** A library a user cannot access is indistinguishable from one that does not exist ([`users.md` §10](users.md#10-access-semantics)).

---

## 5. Staying Current

A library publishes its changes, and clients follow that feed rather than polling or re-fetching.

- **Library changes reach users without them asking.** A newly scanned album appears on every signed-in device; nobody refreshes, and nobody waits for a scheduled sync.
- **Every kind of change propagates** — content added, metadata updated, tracks removed, and tracks going missing or returning.
- **Catching up costs what changed, not what the library holds.** This is what makes carrying a 500,000-track catalog on a phone practical ([`offline.md` §1](offline.md#1-the-catalog-is-always-there), [`performance.md` §6](performance.md#6-client-footprint)).
- **A device that has been away for months recovers correctly**, ending up with an accurate library rather than a stale or partial one.
- **A large import does not flood clients.** Importing ten thousand tracks must not degrade the experience of anyone listening while it happens.

How clients hold and use that catalog is [`offline.md`](offline.md).
