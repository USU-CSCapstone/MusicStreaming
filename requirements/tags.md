# Tag Requirements

## Overview
A tag is a label a file carries about its music. Tags are how a collection is sliced into something browsable beyond artist and album — the answer to "play me something jazzy" rather than "play me this record".

**Genre is the only tag type in v1.** The model is nonetheless general, because the useful additions come later — moods, and eventually personal labels — and a system that can only hold genres would have to be rebuilt to hold anything else.

Shared behavior follows `conventions.md`.

---

## 1. Tags Come From Files

**Tags are read from file metadata and nothing else** (`scanning.md` §2). Jewelcase applies none of its own, infers none from folder names, and offers no interface to add or edit them.

- **There are no user-applied tags in v1.** Enriching or correcting genre data means correcting the files — by the user, or by a plugin.
- **Plugin-written tags are ordinary tags.** A plugin that improves genre metadata writes to the files, and the result is indistinguishable from hand-tagging (`general.md` §3.2).
- **The core never writes to the library** (`general.md` §3.1).

---

## 2. Tag Types

**Every tag has a type.** In v1 there is exactly one — **genre**.

Anticipated but not built: **mood**, from the standard mood field; **personal labels**, if user tagging is added later; and **personal states** such as favourite or hidden, deliberately out of scope for v1 (`analytics.md` §7).

- **Types are independent.** Filtering by one never implicitly filters by another, so a mood filter added later composes with the genre filter rather than replacing it.
- **Adding a type must not change how existing tags behave**, or how users already interact with them.

---

## 3. Multiple Values

- **A track can carry many tags of one type.** Three genres on one track is normal, not a conflict to resolve.
- **Multiple values are preserved, never flattened** into a single string (`scanning.md` §2).
- **Values are semicolon-separated**, the same convention as artists (`artists.md` §1). Tag formats that carry repeated fields natively are read the same way.
- **Order carries no meaning.** Genres are unranked.

---

## 4. Matching

**Tags differing only in case or spacing are the same tag.** `Rock`, `rock`, and `  Rock ` are one.

- **Nothing else is merged.** `Hip-Hop` and `Hip Hop` remain separate, because deciding they are the same means deciding which genres are genuinely distinct — a judgement Jewelcase has no standing to make about someone else's collection.
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
- **An artist's tags come from their work** (`artists.md` §2), reflecting what they actually recorded rather than one label applied to a career.
- **Derived tags are never written back.** An album is not retagged because its tracks were; nothing on disk changes.

---

## 7. Browsing and Filtering

- **Filter by tag anywhere entities are listed** (`conventions.md` §4).
- **A tag has a page** — the artists, albums, and tracks carrying it, browsable and playable like any other collection.
- **Filters compose.** Genre with year, genre with listening data, or several tags at once, narrowing or widening as the user chooses.
- **A filtered view is not a playlist** (`playlists.md` Overview). It leaves nothing behind.
- **Filtering stays instant at full scale** (`performance.md` §3).

---

## 8. Absence

- **Most real libraries are poorly tagged for genre.** That is the normal case, not an error, and never reported as a scan problem.
- **Nothing depends on tags existing.** Browsing, search, statistics, and recommendations all work on a library with no genre tags at all — with less to go on, but never broken.
