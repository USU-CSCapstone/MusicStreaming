# Track Requirements

## Overview
A track is one audio file and everything Jewelcase knows about it — the atomic unit of the system. It is what plays, what playlists hold, what queues order, and what history records.

Every field comes from the file's tags. Jewelcase adds no metadata of its own and offers no way to edit what it read. Discovery and parsing belong to [`scanning.md`](scanning.md); this file describes what a track *is*.

Shared behavior — personal data, actions, presentation, availability, multi-artist display — follows [`conventions.md`](conventions.md).

---

## 1. Identity

A track's identity follows its **content** — not its path and not its tags ([`scanning.md` §6](scanning.md#6-track-identity)).

This matters here because everything points at tracks: playlists, queues, history, statistics. Any of them breaking because a user tidied their folders would be a failure of the whole system.

---

## 2. Core Metadata

- **Title**
- **Artists** — an ordered list ([§3](#3-artist-credits))
- **Album** — with its own artists ([`albums.md`](albums.md))
- **Track number** and **disc number**, with totals where tagged
- **Duration**, precise enough that progress bars, scrobble thresholds, gapless transitions, and playlist totals are exact rather than drifting.
- **Year or release date**, at whatever precision the tag provides

A track missing any of these remains valid. An untagged file is a playable track named after its filename, and every feature handles it without special-casing.

---

## 3. Artist Credits

**Multiple artists are the normal case, not an exception.** Handling them well is a deliberate priority — collaborations and features are common in real collections and consistently mishandled elsewhere.

A track's artists are distinct from its **album's** artists. The relationship between the two determines ownership versus feature credit, per [`artists.md` §2](artists.md#2-ownership-discography-vs-appearances).

Parsing follows [`artists.md` §1](artists.md#1-identity); display follows [`conventions.md` §7](conventions.md#7-multi-artist-handling).

---

## 4. Audio Properties

What a track's audio actually is: **codec and container, bitrate, sample rate, bit depth, channel count, file size.**

- **Users can see the real quality of a track**, so someone holding both a FLAC and an MP3 of one song can tell them apart at a glance.
- **Playback and download decisions rest on these**, so they must be accurate for multi-channel and high-resolution audio rather than assuming stereo.
- **The file's path is recorded but not general-purpose information.** It is needed to serve the file and useful to an admin diagnosing a scan problem; it is not part of what a listener browses, and is not sent to clients that do not need it ([`general.md` §3.7](general.md#37-payloads-carry-only-what-the-client-needs)).

---

## 5. Lyrics

- Both **time-synchronized** and **plain-text** lyrics are supported.
- Synchronized lyrics stay aligned through seeking and pausing.
- Where synchronized lyrics exist, they are the primary view, with plain text as fallback.
- Most tracks have none. Their absence shows a clean empty state, never an error.

Resolution from disk follows [`scanning.md` §3.4](scanning.md#34-lyrics). Lyrics are never authored, edited, or fetched by the core.

---

## 6. Classification & Identifiers

- **Explicit content** flagged from tags, so clients can mark it visibly.
- **Genres** preserved as multiple values ([`tags.md`](tags.md)).
- **ISRC** and other tagged identifiers, stored primarily so plugins can match a track against external services reliably rather than guessing from a title string. Not shown as primary information to listeners.
