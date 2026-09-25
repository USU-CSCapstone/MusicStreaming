# Album Requirements

## Overview
An album is a group of tracks released together. It is not a folder and not a record anyone maintains — it emerges from tags, and exists only because some set of tracks agree about what album they belong to.

Albums are therefore as good as the tags in a collection, and Jewelcase does not paper over the difference. A messy library produces albums that accurately reflect the mess; the fix is to fix the tags.

Shared behavior follows [`conventions.md`](conventions.md).

---

## 1. Identity

**An album is identified by its title together with its album artists.**

- Same title and same album artists means the same album, regardless of location on disk. An album split across two folders, or two drives, is one album.
- Different album titles in one folder are different albums. Folder structure carries no weight.
- **Album artists are a list**, semicolon-separated as track artists are. The full set participates in identity.
- **Editions are distinguished by title, and by nothing else.** An original, a remaster, and a deluxe edition sharing one title and one set of album artists are one album, and their tracks merge. Separating them means tagging them apart — `Nevermind` and `Nevermind (Deluxe Edition)`. Jewelcase reads no edition, catalogue-number, or release-ID tag to do it automatically, because doing so would split albums for users who never asked.

---

## 2. Metadata

- **Title**
- **Album artists** — ordered, as tagged ([`artists.md` §2](artists.md#2-ownership-discography-vs-appearances))
- **Release year or date**, at tag precision
- **Total tracks** and **total discs**, from tags rather than counted from what is present ([§3](#3-type), [§4](#4-track-listing--completeness))
- **Type** ([§3](#3-type))
- **Genres** ([`tags.md`](tags.md))
- **Total duration** and **track count** of the tracks present, shown wherever an album appears

---

## 3. Type

Albums are classified so an artist's discography can be split into meaningful sections.

1. **The release-type tag**, where present, is authoritative.
2. **Otherwise, infer from the tagged total-track count:**

   | Total tracks | Type |
   |---|---|
   | 1–3 | Single |
   | 4–6 | EP |
   | 7 or more | Album |

3. **The compilation flag** independently marks a compilation, overriding size-based inference.
4. **No usable total falls through to Album**, the default (below).

Inference uses **the total-track count from tags, never the number of tracks present in the library.** A user who owns three tracks of a twelve-track album has an incomplete album, not an EP. Inferring from what happens to be on disk would silently reclassify every partial album in a collection and scatter artists' discographies into the wrong sections.

**The default type is Album.** A release with no type tag and no usable total-track count is classified as an album rather than left unclassified — an unclassified release would have nowhere to appear in a discography ([`artists.md` §3.1](artists.md#31-discography)).

---

## 4. Track Listing & Completeness

- Ordered by **disc number, then track number**.
- **Multi-disc albums are presented as discs**, with clear separation, not one flat run of numbers.
- **Incomplete albums are shown as incomplete.** Where the tagged total exceeds what is present, the album says so — a user can tell at a glance they have 3 of 12 — rather than silently renumbering to look complete.
- Incompleteness is **information, not an error**. Partial albums are ordinary in real collections: labeled, never hidden, never flagged as a problem.
- **Missing tracks** appear in their correct position, marked unavailable ([`scanning.md` §8](scanning.md#8-missing-files)).

---

## 5. Artwork

Resolution follows [`scanning.md` §3.1](scanning.md#31-album-art); serving, caching, and placeholders follow [`conventions.md` §5](conventions.md#5-imagery).

Album-specific: where tracks on one album resolve to different artwork, the album presents **a single coherent cover** rather than changing as playback moves between tracks.

---

## 6. Album-Specific Behavior

Beyond [`conventions.md`](conventions.md):

- Browsing additionally supports **filtering by type and year**, and **sorting by release date and duration**.
- Play and shuffle cover the whole album in correct disc-and-track order.
