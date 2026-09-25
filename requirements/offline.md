# Offline Requirements

## Overview
Offline is a first-class mode, not a degraded fallback ([`general.md` §1](general.md#1-goals)). It follows from **offline parity** ([`general.md` §3.5](general.md#35-offline-parity)): the same inputs produce the same queue order, the same sort, and the same navigation with or without a network.

Two differences are permissible, and only these: **whether a track's audio can play**, and **what the device has to search** — a smaller corpus, never a weaker search ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)). Everything else must be identical.

Three things are held on a device, and the distinction runs through this document:

| | What it is | How it gets there |
|---|---|---|
| **Catalog** | Everything *about* the music — titles, artists, albums, numbers, durations, genres, and the user's own playlists, queues, and history | Synced in full, automatically, never asked for |
| **Imagery** | Artwork and artist images | A stand-in for every image, always. The real thing for what the user has seen or downloaded |
| **Audio** | The files themselves | Downloaded deliberately by the user |

The catalog is small enough to carry entirely; imagery and audio are not, and do not pretend to be.

---

## 1. The Catalog Is Always There

**Every library a user can access is browsable, searchable, and queueable on their device without downloading anything** — including tracks whose audio is not present. This is the design target and the normal case; [§1.2](#12-when-the-catalog-does-not-fit) is the only exception.

- **Sync is automatic and invisible.** The user never initiates it, waits on it, or is told a device is "not ready".
- **A partially synced device is fully usable.** Sync fills in progressively; nothing is gated on its completion.
- **It costs proportionally.** Bringing a device current after ten new tracks costs what ten tracks cost, not what the library costs.

### 1.1 Footprint
The device footprint is **bounded well below what the music itself would cost** — even at the 500,000-track target, comparable to a handful of downloaded songs ([`performance.md` §6](performance.md#6-client-footprint)).

- **A visual stand-in for every album and artist**, a few bytes each, enough to render a recognizable, correctly coloured placeholder. A whole library's worth costs about as much as one photograph.
- **Full-resolution imagery is cached as it is seen**, within a bounded cache, and is always present for downloads ([§2](#2-a-download-is-complete)).
- **Lyrics arrive with downloads**, on demand otherwise — too large to carry wholesale, and of no use without the audio. **Downloaded lyrics are indexed and searchable on the device**, so a user who has downloaded everything can search lyrics offline exactly as they would online ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)).
- **Waveforms travel the same way as lyrics** ([`playback.md` §3](playback.md#3-the-waveform)): bundled with a download, fetched on demand for a streamed track, cached once seen. They are per-track data useful only while that track is playing, and at 500,000 tracks would cost more than the whole catalog budget — so they are never pre-synced wholesale. **Every downloaded track carries its waveform**, so offline playback looks identical to online.
- **Similarity data rides along with the catalog**, compact enough to be part of it rather than a second index, so endless play keeps working offline ([`recommendations.md` §10](recommendations.md#10-offline)).

Imagery a user has seen or downloaded is real; the rest is a deliberate placeholder ([`conventions.md` §5](conventions.md#5-imagery)), never a blank grid.

### 1.2 When the Catalog Does Not Fit
The footprint budget ([§1.1](#11-footprint)) is set so this should not happen. Where a device genuinely cannot carry the full catalog anyway, the guarantee that survives is: **anything downloaded remains completely self-sufficient** ([§2](#2-a-download-is-complete)), as do the user's playlists, queues, and history.

Any shortfall is a **bounded and stated** scope — the user is told what their device holds, and search says what it covered ([`search.md` §2.2](search.md#22-saying-what-was-searched)). What must never happen is the device answering a question *less well* than the server would; a smaller catalog is acceptable, a weaker search is not ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)).

---

## 2. A Download Is Complete

**Downloading anything brings everything needed to make it behave exactly as it does online.** An album brings its tracks, artwork, artists, artist images and biographies, lyrics, and credits — so a user can search for one track from it and play it, open its artist and find it there, or follow a featured credit onward. Nothing downloaded is a dead end.

- **Downloads compose upward.** Downloading a track populates the album and artist that own it, so it appears in context rather than alone.
- **Partial containers are honest.** An artist with 1 of 12 albums downloaded says so ([`albums.md` §4](albums.md#4-track-listing--completeness)). Partial is normal, never an error.

---

## 3. What Can Be Downloaded

Any entity, wherever the Download action appears ([`conventions.md` §2](conventions.md#2-actions)): **a track, album, artist, playlist, or entire library.**

**A download is a standing instruction, not a snapshot.** Tracks added to a downloaded playlist download automatically and removed ones free their space ([`playlists.md` §10](playlists.md#10-playlist-specific-behavior)); tracks that scanning adds to a downloaded album, artist, or library are downloaded too.

Removing a download frees its space immediately and **never touches the library on the server.**

Jewelcase downloads nothing the user did not ask for.

---

## 4. Choosing Between Local and Stream

**A downloaded track plays from the device unless streaming would genuinely sound better on the connection at hand.** Offline, or with no local copy, there is no choice to make.

Quality is set per device, separately for unmetered connections, metered connections, and downloads ([`users.md` §6](users.md#6-profile--preferences)).

- **The higher quality wins.** Where the current connection's streaming quality exceeds the local copy's, the stream plays; where it is equal or lower, the local copy does.
- **One preference overrides all of it** — *never stream a track that is downloaded* — for users who would rather spend no bandwidth than gain quality. The choice is made once, not per track.
- **The defaults are already bandwidth-safe.** A metered quality at or below the download quality means the local copy always wins on cellular, unconfigured. A track downloaded at original quality can never be beaten by a stream, so it always plays locally.
- **Falling back always favours playing something.** A stream that degrades reverts to the local copy rather than stalling; an unreadable local copy reverts to streaming and repairs itself; a track still downloading streams rather than making the user wait.
- **Local playback costs the server nothing**, consuming no concurrent stream ([`performance.md` §1](performance.md#1-verification-hardware)).
- **The source is invisible downstream.** Plays, history, statistics, and remote control behave identically either way ([`analytics.md`](analytics.md), [`realtime.md`](realtime.md)).

---

## 5. Storage & Quality

- **The user sets a storage budget** per device, and Jewelcase stays inside it.
- **Nothing is ever evicted automatically.** A download disappears only when the user removes it — an album downloaded for a flight must never be silently reclaimed.
- **Hitting the budget is explained.** A download that does not fit says so, says how much is needed, and offers a route to freeing space, rather than stalling unexplained.
- **A standing download that cannot grow** marks itself incomplete and says why, rather than quietly falling behind.
- **Usage is legible**, broken down enough to decide what to remove.
- **The catalog and image cache sit outside the budget** — the user did not choose them and cannot remove them without breaking offline browsing. Their cost is disclosed, and keeping it small is the project's responsibility ([§1.1](#11-footprint)).

**Changing the download quality ([§4](#4-choosing-between-local-and-stream)) applies to new downloads only.** Re-fetching existing ones at the new quality is an explicit user action, never an automatic re-download.

---

## 6. What Works Offline

Everything, other than audio the device does not hold.

- **Browse, search, sort, and filter** across the full library, with search matching and ranking exactly as the server does ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)).
- **Build and manipulate queues**, including shuffle, over any context ([`queue.md` §4](queue.md#4-shuffle)) — the same context, sorting, and filters produce the same sequence they would online.
- **Create and edit playlists**, reconciling on reconnect ([§8](#8-reconnection)).
- **Play what is downloaded**, with plays and listen time recorded locally ([§8](#8-reconnection)).
- **"Not on this device" and "gone from the server" are different states**, presented differently, and both skipped in place during playback ([`conventions.md` §6](conventions.md#6-availability)).

What genuinely requires a server — administration, scanning, plugin activity, and freshly generated recommendations — is **shown as unavailable, not hidden.** The interface never rearranges itself when connectivity drops.

---

## 7. Crossing the Boundary

- **There is no mode switch.** Losing or regaining a connection changes nothing about how the app is used or where anything is. Playback continues.
- **Connectivity state is visible but unobtrusive** — discoverable when it matters, never a banner demanding acknowledgement.
- **The user can force offline**, protecting a metered connection without disabling the device's network.
- **Reconnection is not an interruption.** No reload, no lost scroll position, no lost queue.

---

## 8. Reconnection

Work done offline is not provisional. It syncs when a connection returns, with no button to press and no state the user must remember to leave a device in.

- **Playlist edits follow the same rule as any other edit** ([`playlists.md` §5](playlists.md#5-concurrent-edits)): applied if the playlist has not changed meanwhile, refused with a plain explanation if it has.
- **Plays reconcile per [`analytics.md` §6](analytics.md#6-offline-and-reconciliation)** — timestamped when they happened, never when they synced, and never counted twice however many times a device reconnects.
- **Failures are surfaced once, clearly, and are recoverable** — never a silent drop, never a permanently stuck backlog.

---

## 9. Download Lifecycle

- **A downloaded track whose file leaves the server keeps playing on that device**, marked as no longer in the library. It stops counting against server retention ([`scanning.md` §8](scanning.md#8-missing-files)).
- **Retagging updates metadata, not audio.** Identity follows content ([`scanning.md` §6](scanning.md#6-track-identity)), so a retagged or moved file is the same download.
- **Replaced audio re-downloads.**
- **Losing access to a library removes its downloads from the device** — access is the boundary ([`general.md` §3.6](general.md#36-libraries-are-the-isolation-boundary)). Playlists, history, and statistics survive and return if access is restored ([`users.md` §5](users.md#5-library-access)).
- **Signing a device out removes its downloads**, as does deleting the account ([`users.md` §9](users.md#9-account-lifecycle)).
- **A password change is not that case.** Changing a password signs other devices out ([`users.md` §3](users.md#3-authentication)), but those devices **keep their downloads**, usable again the moment the same account signs back in. The same holds for an admin-initiated reset. A user securing their account must not pay for it by re-downloading their library over cellular.

---

## 10. Visibility & Control

- **Downloads belong to a device**, not an account.
- **Download state is shown wherever the entity appears** — downloaded, partial, downloading, queued, or not — in one consistent vocabulary.
- **Progress is real**: what is downloading, how far along, what remains.
- **Downloads survive interruption**, resuming rather than restarting after app closure, signal loss, or device restart.
- **Downloads yield to listening** ([`performance.md`](performance.md)).

---

## 11. Platform Parity

**Offline audio works on the responsive web app and the Android app alike.** iOS reaches Jewelcase only through the PWA ([`general.md` §4](general.md#4-target-platforms)), so anything less means no offline listening on iPhone at all.

- **Both platforms hold audio durably**, surviving app closure and device restart.
- **Background download and background playback** work on both.
- **Where a platform imposes a real limit**, the interface states it plainly and enforces the budget honestly rather than accepting downloads it cannot keep.
