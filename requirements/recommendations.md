# Recommendation Requirements

## Overview
Recommendations are how a user gets back into a collection too large to browse. In a self-hosted library the problem is not finding new music — it is that people own far more than they remember owning, and most of it is never played again after the week it was imported.

Two things are known about the music, and recommendations rest on both:

| | What it knows | What it reaches |
|---|---|---|
| **Listening** | What a user plays, finishes, repeats, and skips (`analytics.md` §2) | Music with a history |
| **Sound** | What each track actually sounds like, measured during scanning (§2.1) | Everything, including what has never been played and never been tagged |

Neither is sufficient alone. Listening data cannot reach the 400,000 tracks a user has never touched; sound cannot tell that two records belong together for reasons that have nothing to do with how they sound.

Shared behavior follows `conventions.md`.

---

## 1. What Can Be Recommended

**Only music in the library.** Every recommendation is playable the moment it appears — one tap, no dead ends, nothing a user is shown and then denied.

- **Missing and unavailable tracks are never recommended** (`scanning.md` §8).
- **Pointing outside the library is a plugin's job.** "Artists like this that you do not own" is a legitimate feature and an illegitimate core one, because it turns a music player into a shopping list and requires an external service the core does not need (`general.md` §2, `plugins.md` §10).
- **Recommendations are scoped to one library** (`general.md` §3.6). What a user hears in one is never influenced by another.

---

## 2. What It Learns From

### 2.1 How the Music Sounds

**The scanner derives a description of every track's sound** — its tempo, key, energy, and overall sonic character — as part of analyzing it (`scanning.md` §1).

This is what makes recommendations work on a real collection:

- **It needs no tags.** Most libraries are badly tagged for genre (`tags.md` §8), and a system that depended on genre would fail on exactly the collections that need help most.
- **It needs no history.** A track imported an hour ago is as reachable as one played a thousand times. This is the answer to a new user, a new library, and the long tail of owned-but-unheard music.
- **Its absence degrades, never breaks.** Analysis runs behind scanning and takes days at full scale (`performance.md` §5); until a track has it, that track is recommended on listening data alone.

### 2.2 How the User Listens

- **Completion, not play count** (`analytics.md` §2). A track started thirty times and skipped thirty times is evidence of dislike, and treating it as a favourite is the single easiest way to make recommendations feel broken.
- **What is played together.** Tracks a user reaches in the same sittings belong together, whatever they sound like.
- **When it is played.** Time of day and day of week are real patterns (`analytics.md` §4), and a user's morning music is rarely their evening music.
- **Skips are the only "no" a user has.** With no hiding and no ratings (`analytics.md` §7), a repeatedly skipped track must stop appearing quickly. Nothing else in the system lets a user reject anything.

### 2.3 What Is Not Used

- **Other users' listening is never a signal.** Listening data is private between accounts (`analytics.md` §9), and a household of three is too small a population to learn from regardless.
- **Genre tags contribute where they exist and are never required** (`tags.md` §8).

---

## 3. Similarity

**Similarity is one measure combining sound and behaviour**, used by every surface below, so "more like this" means the same thing everywhere it appears.

- **Sound and behaviour are weighed together.** Acoustic likeness alone produces confident nonsense — a ballad and an unrelated ballad — and behaviour alone cannot reach unplayed music. Each covers the other's blind spot.
- **Evidence outweighs resemblance.** Where a user's own listening says two things belong together, that beats an acoustic guess.
- **Similarity is computed over the whole library**, not over what a user has played.
- **It is stable.** The same seed produces recognizably the same neighbourhood from one day to the next; a library that recommends something different every time it is asked is not learning, it is shuffling.

---

## 4. Radio & Endless Play

**Radio starts from anything and keeps going** — a track, album, artist, playlist, or genre.

- **It never ends.** Radio does not run out at fifty tracks; it continues for as long as the user listens (`general.md` §3.3).
- **It starts close and widens.** The first tracks are unmistakably related to the seed; the further it runs, the further it may travel.
- **It becomes the context** and appears in Up Next like any other queue (`queue.md` §1), fully reorderable and removable.
- **Endless play is radio seeded by what just finished** (`queue.md` §5), so a user who starts an album and walks away keeps listening in the same vein.
- **Automatic tracks are always marked as such** (`queue.md` §5). A user must never wonder why something is playing.
- **Nothing repeats within a session** (`queue.md` §4).

---

## 5. Generated Mixes

Rotating playlists built for the user, appearing among their playlists and behaving like any other (`playlists.md` Overview).

- **Each has a reason to exist** — an artist and its neighbourhood, a decade, a mood of the collection, a time of day. A mix a user cannot describe in a phrase is a random shuffle with a title.
- **Fewer, better mixes.** Mixes are generated only where there is enough evidence to build a good one. A small library gets three real mixes, not ten padded ones.
- **They regenerate on a predictable cadence** and are **stable in between** (`playlists.md` §8) — a user can play one, leave, and come back to the same playlist.
- **A replaced mix is gone.** Users are told a mix refreshes, and saving it is how they keep it (`playlists.md` §8).
- **Each mix is named for what it is**, not "Mix 3".

---

## 6. On Entity Pages

- **Similar artists on an artist page**, similar albums on an album page, similar tracks on a track (`artists.md`, `albums.md`, `tracks.md`).
- **Drawn from the library and playable in place** (`conventions.md` §2).
- **Absent rather than padded.** A library with nothing genuinely similar shows nothing, instead of filling the row with weak matches to avoid an empty space.

---

## 7. Rediscovery

The surface that matters most in a large owned collection, built on what `analytics.md` §4 already records:

- **Loved once, gone quiet** — music with strong completion that has not been played in a long time.
- **Owned and never heard** — tracks in the library with no plays at all, reachable only because sound analysis does not need history (§2.1).
- **Anniversaries** — what a user was listening to a year ago.

---

## 8. Rules Every Surface Follows

- **A recommendation is not a leaderboard.** A surface that returns a user's most-played tracks has told them nothing they did not know.
- **The familiar and the forgotten are balanced.** All-familiar is pointless; all-unfamiliar is exhausting.
- **Nothing recently played is recommended back.**
- **Every recommendation explains itself** — "because you listened to", "similar to", "you have not heard this in two years". Without any way to like or hide (`analytics.md` §7), an explanation is the only thing that makes a bad recommendation legible instead of arbitrary.
- **Recommendations are actionable in place** — playable, queueable, addable to a playlist (`conventions.md` §2).
- **They are fast.** Recommendation surfaces meet the same interaction budgets as anything else (`performance.md` §3); computing them is the server's problem, not the user's wait.

---

## 9. A New User and a Small Library

**Recommendations work on day one, with no listening history at all.** A user who has just signed in is the normal starting case, not an edge case, and an empty discovery screen is the worst first impression the product can make.

- **Sound analysis carries the cold start** (§2.1), giving a coherent radio from any seed before a single play is recorded.
- **The library itself is a signal** — what a user chose to own, what they own most of, what they own complete.
- **Surfaces appear as they become meaningful**, rather than all at once and empty.
- **A small library gets fewer surfaces, not worse ones.** Five hundred tracks cannot support daily mixes, and pretending otherwise produces the same fifty songs under different names.

---

## 10. Offline

- **Endless play works offline.** A queue that ends offline continues from downloaded tracks (`offline.md` §6), because queue behavior is deterministic on either side of the connectivity boundary (`general.md` §3.5).
- **Enough similarity data travels with the catalog** to make that choice on the device — sized like the rest of the offline catalog, in the same footprint budget (`offline.md` §1.1, `performance.md` §6).
- **Generated mixes sync and remain readable offline**, playing whichever of their tracks are downloaded (`offline.md` §6).
- **Radio offline draws on downloaded tracks**, and says so where the choice is visibly narrower.
- **New recommendations are computed on the server.** Offline surfaces the last ones synced rather than nothing.

---

## 11. Privacy and Control

- **Recommendations are personal and private** (`conventions.md` §1, `users.md` §7), derived entirely from one account's own behaviour.
- **Clearing history clears its influence** (`analytics.md` §9). A deleted play must not survive inside a recommendation derived from it.
- **Recommendation surfaces can be turned off** by the user, without disabling radio or endless play, which are things they ask for directly.
