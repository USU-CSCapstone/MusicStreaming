# Search Requirements

## Overview
Search is how a user reaches anything in a large library without navigating to it. At 500,000 tracks it is the primary way in, not a convenience — and it must feel like the search in a commercial streaming service, not like a database query.

**The server searches when it is reachable; the device searches when it is not.** Preferring the server is an efficiency choice — it spares the device the work and the battery, and it always holds the complete library. It is not a quality choice. **The two searches are the same search**, and the device's is not a lesser version of it ([§2](#2-where-search-runs)).

---

## 1. What Is Searched

- **Names** — tracks, albums, artists, and playlists.
- **Lyrics** — find a song by a line half-remembered.

Nothing else in the library. File paths and folder names are **not** searched: the path carries no meaning in Jewelcase ([`scanning.md` §2](scanning.md#2-tags-are-the-only-truth)), and matching on it would produce results a user cannot explain.

**External sources, where a plugin provides one, are searched too** — returned in their own clearly labelled section, never mixed into the sections describing what the user owns ([`plugins.md` §10.2](plugins.md#102-it-behaves-like-music)).

---

## 2. Where Search Runs

- **The server searches whenever it is reachable.** It always holds the complete library and every lyric, and it does the work on hardware that is plugged in.
- **A device still syncing is never worse off.** Search covers the whole library from the first sign-in, long before any catalog sync finishes ([`offline.md` §1](offline.md#1-the-catalog-is-always-there)).
- **The device searches when the server is not.** It covers the catalog it holds and the lyrics of tracks it has downloaded ([`offline.md` §1.1](offline.md#11-footprint), [§2](offline.md#2-a-download-is-complete)).

### 2.1 One Search, Two Places to Run It

**The only thing that may differ offline is what there is to search.** Everything else is identical, and this is a requirement rather than an aspiration — it is the specific way client search is likely to be built wrong.

- **Capability is identical.** The device does the same fuzzy matching, typo tolerance, diacritic folding, word-order independence, and exact-match precedence as the server ([§3](#3-matching)). No simplification, no prefix-only matching, no "good enough for offline".
- **Ranking is identical.** Same top result, same section ordering, same tie-breaking ([§4](#4-results)). Given the same corpus, the two produce the same ordered list.
- **Presentation is identical.** Same sections, same lyric context lines, same actions in place ([§4](#4-results)).
- **A fully synced device returns exactly what the server would.** Where a user has downloaded their entire library, they hold every track and every lyric, so **there is nothing left to differ and offline results must match the server's exactly.** This is the case that proves the rule: if the two ever disagree here, client search has been built weaker than the requirement.

### 2.2 Saying What Was Searched

- **A shortfall is stated, never silent.** Where the device lacks catalog or lyrics the server has, the user is told what was covered — not left wondering why a track they own did not appear.
- **Say it only when it is true.** A device holding the full library and its lyrics has nothing to disclose, and must not display a disclaimer implying its results are partial when they are complete.
- **A shortfall is never presented as degradation.** The message is about *coverage* — "lyrics for tracks you have not downloaded were not searched" — never about quality or reduced functionality.

---

## 3. Matching

**Search must be as forgiving as any commercial streaming service.** `tylor sweft` finds Taylor Swift. Anything less will be read as broken, and correctly so.

- **Typos do not get in the way** — transposed, missing, doubled, and wrong letters still find the right thing.
- **Accents and diacritics are irrelevant.** `bjork` finds Björk.
- **Case is irrelevant.**
- **Partial input matches.** Results appear from the first character and narrow as the user types. Nobody finishes a word or presses enter.
- **Word order is irrelevant.** `swift taylor` finds Taylor Swift.
- **Leading articles do not matter.** `beatles` finds The Beatles ([`conventions.md` §4](conventions.md#4-sorting--browsing)).
- **Exact matches always win.** Forgiveness must never bury a perfect match beneath an approximate one — this is the guardrail that keeps fuzzy matching from feeling random.

---

## 4. Results

**The single best match is shown first, on its own**, above everything else and large enough to act on immediately. Searching `Say It Ain't So` puts that track at the top, ready to play.

- **The top result also appears in its section.** It is a shortcut, not a relocation.
- **Everything else is grouped by type** — tracks, albums, artists, playlists, and lyric matches — each showing its strongest results.
- **Sections are ordered by how well they match.** A query that overwhelmingly matches artists puts artists first.
- **Ties break toward the more specific thing: tracks, then albums, then artists.** Searching the name of a single shows the track before the release that shares its name.
- **Lyric matches show their line in context**, so a user sees the words they remembered.
- **Each section expands to a full list**, sortable per [`conventions.md` §4](conventions.md#4-sorting--browsing).
- **Every result is actionable in place** — playable, queueable, addable to a playlist — without opening it first ([`conventions.md` §2](conventions.md#2-actions)).

---

## 5. Scope

**Search is scoped to the current library.** Results come from the library the user is in, and v1 clients never mix libraries in one set of results.

- **Switching library switches the search.** Users move between libraries freely ([`users.md` §5](users.md#5-library-access)), and search follows without being told to.
- **Cross-library search is supported but not surfaced in v1.** The capability exists for later versions and for third-party clients ([`general.md` §4](general.md#4-target-platforms)). The shipped apps keep to one library, because merged results raise "which library is this from?" on every row.
- **Never beyond access.** Content a user cannot reach is indistinguishable from content that does not exist ([`users.md` §10](users.md#10-access-semantics)).
- **Narrowable to a container** — within an artist, an album, or a playlist, without leaving it ([`playlists.md` §7](playlists.md#7-organization)).
- **Filterable by type**, so someone looking only for albums is not shown tracks.
- **Composable with filters** — genre, year, and listening data ([`tags.md` §7](tags.md#7-browsing-and-filtering)).

---

## 6. Recent Searches

- **Recent searches are offered when the box is empty**, so repeating yesterday's search costs nothing.
- **Individually removable, and clearable entirely.**
- **Private to the user** ([`users.md` §7](users.md#7-privacy--personal-data)), and kept only for a recent window.
- **There is no permanent search history** — nothing to browse, nothing to audit, nothing to leak.

---

## 7. Speed

- **Results within 50 ms at full scale** ([`performance.md` §3](performance.md#3-interaction-budgets)), measured on both verification machines — and **on the device, against the catalog it holds** ([`performance.md` §6](performance.md#6-client-footprint)). Client search meets the same budget as server search; parity of capability ([§2.1](#21-one-search-two-places-to-run-it)) is worth nothing if the device takes a second to deliver it.
- **Results update as the user types**, with no submit step and no perceptible lag between a keystroke and its result.
- **Typing is never blocked.** A slow or failed result must never delay the next character.
- **Results never arrive out of order.** A slower earlier query must not replace a faster later one; a user always sees results for what is currently in the box.
- **A slow or unreachable server hands off rather than hanging.** The device answers instead, to the same standard ([§2.1](#21-one-search-two-places-to-run-it)), and no search ever ends in a spinner that never resolves.
- **Library size does not show.** 500,000 tracks measures the same as 500.

---

## 8. When Nothing Is Found

- **An empty result is a clean, deliberate state** — never an error, never a blank screen.
- **Near misses are offered** where a query nearly matched something, since a user who mistyped badly still meant something.
- **An empty library says so distinctly** from a search that found nothing. "You have no music yet" and "no music matches this" call for different responses.
