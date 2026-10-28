# Tag Requirements

## Overview
A tag is a label a file carries about its music. Tags are how a collection is sliced into something browsable beyond artist and album — the answer to "play me something jazzy" rather than "play me this record".

**Genre and mood are the tag types in v1, and the set is open.** The useful additions keep coming — other fields collections carry, and eventually personal labels — and a system with a fixed list of types would have to be rebuilt for each one.

Shared behavior follows [`conventions.md`](conventions.md).

---

## 1. Tags Come From Files

**Tags are read from file metadata and nothing else** ([`scanning.md` §2](scanning.md#2-tags-are-the-only-truth)). Jewelcase applies none of its own, infers none from folder names, and offers no interface to add or edit them.

- **There are no user-applied tags in v1.** Enriching or correcting genre or mood data means correcting the files — by the user, or by a plugin.
- **Plugin-written tags are ordinary tags.** A plugin that improves genre metadata writes to the files, and the result is indistinguishable from hand-tagging ([`general.md` §3.2](general.md#32-the-scanner-is-the-only-ingestion-path)).
- **The core never writes to the library** ([`general.md` §3.1](general.md#31-the-music-library-is-read-only-to-core)).

---

## 2. Tag Types

**Every tag has a type.** In v1 there are two — **genre** and **mood**, each read from its standard field.

**A type is a field, and nothing more.** Every type is read, matched, and browsed by the same rules — §3 to §8 apply unchanged — and none is special-cased by name. A new type is therefore a new field to read, not new behaviour to build, and genre is the first entry in the list rather than the shape of the list.

Anticipated but not built: **personal labels**, if user tagging is added later; and **personal states** such as favourite or hidden, deliberately out of scope for v1 ([`analytics.md` §7](analytics.md#7-no-explicit-signals-in-v1)).

- **Types are independent.** Filtering by one never implicitly filters by another, so a mood filter composes with the genre filter rather than replacing it.
- **Adding a type must not change how existing tags behave**, or how users already interact with them.
- **Adding a type reaches music already scanned.** Files already in the library show the new type's values without an admin rescanning anything. That costs reading every file's tags once more in the background — a cold scan's reading, without its analysis — and it is worth it: a type that appeared only on files changed since it was added would make most of the library look untagged — the absence of §8, invented by Jewelcase rather than found in the collection.

---

## 3. Multiple Values

- **A track can carry many tags of one type.** Three genres on one track is normal, not a conflict to resolve.
- **Multiple values are preserved, never flattened** into a single string ([`scanning.md` §2](scanning.md#2-tags-are-the-only-truth)).
- **Values are semicolon-separated**, the same convention as artists ([`artists.md` §1](artists.md#1-identity)). Tag formats that carry repeated fields natively are read the same way.
- **Order carries no meaning.** Tags are unranked.

---

## 4. Matching

**Tags differing only in case, or in surrounding or repeated spaces, are the same tag.** `Rock`, `rock`, and `  Rock ` are one, and a tab or no-break space counts as a space.

- **Nothing else is merged.** `Hip-Hop`, `HipHop` and `Hip Hop` remain separate, because deciding they are the same means deciding which genres are genuinely distinct — a judgement Jewelcase has no standing to make about someone else's collection.
- **Display follows the collection.** Where variants of one tag exist, the form shown is the one the library uses most, not an invented canonical spelling.
- **A messy library shows as messy.** Near-duplicate tags stay visible: that is information about the collection, and the fix is to fix the tags.

---

## 5. Flat, Not Hierarchical

**Tags are a flat set with no relationships between them.** `Death Metal` and `Metal` are separate and unrelated tags.

- **No taxonomy ships.** Any genre hierarchy is a contestable claim about music, and maintaining one is a permanent argument with users about their own libraries.
- **No aliasing.** `Hip-Hop` and `Rap` are distinct if the files say so.
- **Browsing a tag shows exactly what carries it**, never a widened set the user did not ask for.

---

## 6. Where Tags Attach

- **Tracks carry tags**, read from their own files.
- **An album's tags are those of its tracks**, so an album is reachable by any genre its tracks carry.
- **An artist's tags come from their work** ([`artists.md` §2](artists.md#2-ownership-discography-vs-appearances)), reflecting what they actually recorded rather than one label applied to a career.
- **Derived tags are never written back.** An album is not retagged because its tracks were; nothing on disk changes.

---

## 7. Browsing and Filtering

- **Filter by tag anywhere entities are listed** ([`conventions.md` §4](conventions.md#4-sorting--browsing)).
- **A tag has a page** — the artists, albums, and tracks carrying it, browsable and playable like any other collection.
- **Filters compose.** Genre with mood, genre with year, genre with listening data, or several tags at once, narrowing or widening as the user chooses.
- **A filtered view is not a playlist** ([`playlists.md`](playlists.md) Overview). It leaves nothing behind.
- **Filtering stays instant at full scale** ([`performance.md` §3](performance.md#3-interactions)).

---

## 8. Absence

- **Most real libraries are poorly tagged for genre, and barely tagged for mood at all.** That is the normal case, not an error, and never reported as a scan problem.
- **Nothing depends on tags existing.** Browsing, search, statistics, and recommendations all work on a library with no tags of any type — with less to go on, but never broken.
