# Artist Requirements

## Overview
An artist is a name that appears in tags and everything that name connects to. Like albums, artists emerge from the collection rather than being records anyone maintains.

The central problem this file solves is **multiple artists on one piece of music**. Collaborations, features, split credits, and various-artist releases are ordinary in any real collection and are consistently handled badly by self-hosted music software — features get mangled into artist names, collaborations create phantom artists, and a guest appearance pollutes someone's discography. §2 is the most important section here.

Shared behavior follows `conventions.md`.

---

## 1. Identity

- An artist is identified by **name**, drawn from artist and album-artist tags.
- Tags hold one or more names **separated by semicolons**; each becomes a separate artist. `Kendrick Lamar;SZA` is two artists, never one artist called "Kendrick Lamar;SZA". Tag formats that carry repeated fields natively are read the same way (`tags.md` §3).
- **The semicolon is the only delimiter.** No inference from "feat.", "ft.", "&", "vs.", or commas. Splitting on those would corrupt correctly-tagged collections in order to rescue badly-tagged ones — "Earth, Wind & Fire" and "Simon & Garfunkel" are single artists whose names contain punctuation.
- **Order is meaningful**; the first name is the primary credit.
- Artists sharing a name are the same artist. Distinguishing two different bands with one name needs information tags do not reliably carry.

---

## 2. Ownership: Discography vs. Appearances

**Whether an artist owns music or merely appears on it is determined by the album-artist tag.**

> An artist **owns** the music on an album when they are among that album's **album artists**. An artist credited on a track who is *not* among the album's album artists is **featured** on it.

| Track artists | Album artists | Result |
|---|---|---|
| `A;B` | `A` | Track is in **A's discography**. **B is featured** — it appears under B's appearances, not among B's own work. |
| `A;B` | `A;B` | Track is in **both discographies**. A genuine collaborative release counts fully for both. |
| `C` | `Various Artists` | Album belongs to **Various Artists**. **C is featured.** |

Consequences the rest of the system must honor:

- **A track can be owned by more than one artist.** Nothing may assume a single "the" artist.
- **A guest appearance never pollutes a discography.**
- **Featured work is never hidden** — it is prominent on the artist's page, just in its own section.
- **The rule needs no heuristics.** It reads two tags and compares them. There is nothing to tune and nothing to guess wrong.

---

## 3. Artist Page

### 3.1 Discography
Albums where the artist is among the album artists, **split by type** — Albums, EPs, Singles, Compilations (`albums.md` §3). Empty sections are omitted. Within each, releases sort by date, newest first, with other sort options available.

### 3.2 Appearances
Tracks where the artist is credited but is not an album artist of the release.

- Its own **clearly-labeled section**, never merged into the discography.
- **Grouped by the album** the tracks come from, so a listener sees which record a guest appearance is on rather than a flat list of orphaned tracks.
- Covers features, compilation contributions, and soundtrack appearances.

### 3.3 Top Tracks
The artist's most-played tracks, ranked by **the viewing user's own play counts** — a personal library's value is personal. Includes owned and featured tracks, marked so the difference is visible. An artist the user has never played still shows a sensible ordering rather than an empty section.

### 3.4 Related Artists
Derivation is defined in `recommendations.md`. Two constraints regardless of method:

- **Related artists never leave the library.** Every suggestion is an artist the user actually has music by.
- **Collaborators surface naturally** — an artist frequently credited alongside this one is a relationship the collection already contains.

---

## 4. Images & Biography

Resolution follows `scanning.md` §3.2–3.3; serving, caching, and placeholders follow `conventions.md` §5.

- **Both are optional and usually absent.** A fresh library has neither, and an artist with no image and no biography must look complete and intentional rather than broken. This is the common case.
- **This is the plugin system's most visible payoff** — install an enrichment plugin and artist pages fill in through the ordinary scan path, with no separate import step.

---

## 5. Artist-Specific Behavior

Beyond `conventions.md`:

- **Every artist parsed from a semicolon-separated tag is independently browsable**, whether credited first, last, or only ever as a guest.
- Play counts and last-played **aggregate across owned and featured tracks**.
