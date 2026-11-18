# Search Design

## Overview
How a search finds and ranks what it finds: names, as built, and lyrics, as decided here. It builds on [`requirements/search.md`](../requirements/search.md) and the shared core ([`general.md` §3](general.md#3-shared-core)); where it conflicts with [`requirements/`](../requirements/), the requirements win.

**The one idea: one search, in the core, over indexes the server and a device build alike, so a device holding what the server holds finds exactly what it finds** ([`requirements/search.md` §2.1](../requirements/search.md#21-one-search-two-places-to-run-it)). Nothing about matching or ranking is the server's own: the server reads rows into the core's index, asks it, and turns IDs back into rows.

---

## 1. Names

Built, and described where it is built: matching and ranking in `crates/core/src/search.rs`, which says its rules in full; the server's indexes and their freshness in `crates/server/src/api/search/indexes.rs` and [`database.md` §4](database.md#4-pagination).

- **A library's names are one index in memory**, rebuilt in the background when its names version moves, which only what search indexes moves: names, and a tag's track count. A user's playlists are a small index built for each search and searched with the library's as one (`Union`), so no one else's is ever in it.
- **A section is a kind, and for tags a type of tag.** Sections order by their best match, ties to the more specific kind; within one, by how well each matched, then weight (a tag's tracks), then folded name, then ID.
- **Paging continues after a match's rank**, its place written out in values, so a rebuilt index neither repeats nor skips. A sorted section leads with its matches when few and walks its list's index when many.
- **Narrowing** by tag or container, and leaving out what the caller hid, is a set of IDs each kind's matches are checked against, kept until the library's change feed moves.
- **Near misses** match again more loosely when nothing matched: a wrong first letter, a typo more, one word left unmatched.

---

## 2. Lyrics

**A lyric match is a track, found by one of its lines that holds every word of the query, shown with that line** ([`requirements/search.md` §1](../requirements/search.md#1-what-is-searched), [§4](../requirements/search.md#4-results)). This section answers Stage 8's open question 3, with the decisions §3 records; the core matches and ranks (`crates/core/src/search/lyric_query.rs`), and the server's `lyrics` section shows the matches (`crates/server/src/api/search/lyrics.rs`).

### 2.1 What is indexed

- **A line is the unit.** Synced lyrics are already lines (`SyncedLine`); plain lyrics are split at line breaks. Blank lines, and section labels that are a line of their own in brackets (`[Chorus]`), are not lines. A line is folded into words exactly as a name is (`words`), so `aint` finds `Ain't` in a lyric as in a title.
- **Each distinct line of a track once.** A chorus sung four times is one line, kept where it first comes, since a track is one result however often its words recur. Lyrics repeat a great deal, so this is most of what keeps the index small (§2.5).
- **Only tracks that have lyrics**, which is most tracks' absence costing nothing. What the caller hid, and what a tag filter or `within` leaves out, are left out as a names search leaves them out, by the same sets.

### 2.2 Matching

- **Every word of the query matches a word of one line**, in any order, as a name's words must: exactly, as a prefix (the query's last word, as it is being typed), or with the typos a word of its length allows, first letter right. A leading article in a query of more than one word may go unmatched, as for names.
- **A query of fewer than three characters searches no lyrics**, its words' characters counted: a first letter or two are in about half of a large library's lines, which costs the most to rank (§2.3) and says the least, so the section is left out until a third is typed.
- **One line, not a span of lines.** A remembered phrase that crosses a line break is the case this misses; matching across two adjacent lines would catch it at the cost of more candidates to check for every query and a larger index. One line, and the second only if it is missed in use.

### 2.3 Ranking

- **Within the section, a track by its best line**, lines ranked in two steps, since the index knows which words a line has but not their order (§2.5):
  1. **Every line that matches, by what the index knows**: fewer typos, the query's leading article matched, fewer words matched only as a prefix, fewer words in the line beyond those matched (a shorter line is the likelier one meant), then the track's ID and the line's place.
  2. **The best 200 tracks of those, their lyrics read back from where they are stored, with the tracks that have a line holding the query's words in order and together first** (the phrase remembered), each by its best such line, which may not be the one step 1 chose, ranked as step 1 ranks lines; the rest keep their order after them. A leading article that may go unmatched is not part of the phrase. A query of one word, which every matching line holds in order, reads nothing back.

  The server keeps the last few searches' step 1 matches, everything they found, while the index they were matched in stands, and narrows them to what each caller's search keeps: a page after the first, the Lyrics tab, or the same words again match no line again. Step 2 runs for each, on what the caller keeps.

  A device does the same over its own stored lyrics, so the two rank alike. A phrase only past the first 200 tracks is not lifted; in practice the lines that hold a phrase are among the shortest and least mistyped, so step 1 already keeps them near the top. At the heavy case, step 1 takes 0.2 ms for a rare word and 30–50 ms on an M5 for the commonest words and first letters, which hold up to half the lines; step 2 takes 6 ms more, given the lyrics read.
- **Among the sections, a lyric section never outranks a name matched exactly** ([`requirements/search.md` §4](../requirements/search.md#4-results)): it sorts as if its best match were never exact, so a section whose best is an exact name comes first, and against inexact names it is compared as they are. So searching a title shows the song's name before its chorus, and searching a line no title holds puts lyrics first.
- **A lyric can be the top result**, when its section leads: typing a half-remembered line puts that song at the top, ready to play, with the line shown. Otherwise the top result is a name, as now.

### 2.4 What a match shows

- **The line, one line before it and one after**, as `LyricMatch` names them, read from the stored lyrics for the matches shown, not kept in the index. A synced line carries its `startMs`, a plain one none.
- **Playing a match plays the song from its start**, as any song plays from a list; the line is shown, not played from. Starting at `startMs` would need the queue to start partway through a track, which it does not, or a seek once it has started.
- **Lyrics change on their own**: a plugin adds them, a scan finds an `.lrc`. They get **a version of their own**, which triggers on `track_lyrics` move, so a lyric written rebuilds only the lyric index and a renamed track only the names one.

### 2.5 Memory, on the server

**An index in memory, as names are, with compact postings, under 100 MB for the heavy case: 76 MB, measured.** The heavy case is 150,000 tracks with lyrics, each 30 lines of its own and a chorus of five sung three times, so 45 lines and 35 distinct, of 4 to 10 words from a 60,000-word vocabulary that occurs as a language's does (`benchmark_lyrics_full_scale`). Its 5.25 million distinct lines build in 7 s on an M5, a track at a time, so building holds 144 MB at its peak, the index and its postings' spare room, however many lyrics are read: the server reads them a batch at a time.

| What | Measured |
|---|---|
| Postings: each word's lines, as variable-length gaps | 57.2 MB |
| Each line's place among its track's lines | 10.5 MB |
| Each line's count of words | 5.2 MB |
| Each track's ID and first line | 1.8 MB |
| Vocabulary | 1.4 MB |

- **Each line's words, in order, are not kept**: with where each line starts, they measured 79 MB, and would have put the index at 150 MB. Order is needed only to lift the lines that hold the query as a phrase, which reading the best 200 lines back does (§2.3).
- A typical library has lyrics for far fewer tracks, and the index costs nothing for a track with none.
- **Over 100 MB**, the same core matching would read its postings from the database instead of memory, keeping only the vocabulary resident. Searching lyrics with SQLite's own full-text search is ruled out either way: its words and ranking are not the core's, so the device, which has no such engine, could never find what the server finds.

### 2.6 On a device

- **A device indexes the lyrics of the tracks it has downloaded** ([`requirements/offline.md` §1](../requirements/offline.md#1-the-catalog-is-always-there)), with the same core, so with everything downloaded it holds every lyric and finds exactly what the server finds. Its index counts against the downloads, not the catalog ([`requirements/performance.md` §6](../requirements/performance.md#6-client-footprint)).
- **It says what it did not search**: "lyrics for tracks you have not downloaded were not searched", only when some were not ([`requirements/search.md` §2.2](../requirements/search.md#22-saying-what-was-searched)). That is Stage 10's, with the rest of offline search.

---

## 3. Decisions

Made for lyrics, before they are built:

1. **The server's lyric index is in memory**, with compact postings, as names are, under **100 MB** for 150,000 tracks with lyrics. Over budget at full scale, its postings move to the database (§2.5).
2. **A match is one line**; not across a line break (§2.2).
3. **A lyric can be the top result** when its section leads (§2.3).
4. **One line of context** before and after the match (§2.4).
5. **A match plays from the start** of its song (§2.4).
