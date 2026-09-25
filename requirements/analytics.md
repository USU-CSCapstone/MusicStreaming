# Analytics Requirements

## Overview
Jewelcase records what a user listens to, so they can look back on it and so the system can learn what they like ([`recommendations.md`](recommendations.md)).

The recording rule is deliberately simple and the interpretation is where the intelligence lives: **a play is any track that starts, and listen time is how much of it was actually heard.** Nothing is judged at the moment of recording — a 4-second sample and a full listen are both plays, distinguished afterward by how long they lasted. Simple to record means unambiguous, verifiable, and never quietly wrong.

Listening data is **personal and private** ([`conventions.md` §1](conventions.md#1-personal-data), [`users.md` §7](users.md#7-privacy--personal-data)).

---

## 1. What Is Recorded

**Every track that begins playing is a play.** No threshold, no minimum duration, no judgement at the point of recording.

Each play records:

- **The track**, and **when** it started.
- **Listen time** — how much audio was actually heard, accumulating as it plays.
- **How it ended** — finished, skipped, or stopped.
- **The context** it was played from, as defined in [`queue.md`](queue.md) — including radio and endless play, which are contexts like any other.
- **The device** it played on ([`users.md` §4](users.md#4-sessions--devices)).

Rules that keep the record honest:

- **Listen time is audio heard, not wall-clock time.** Pausing does not accumulate it; seeking backwards to replay a chorus does; skipping forward does not.
- **Listen time can exceed a track's duration**, and that is information rather than an error — someone replayed part of it.
- **Repeat produces separate plays**, one per pass, rather than one long play.
- **Local playback is recorded identically** to streamed playback ([`offline.md` §4](offline.md#4-choosing-between-local-and-stream)).
- **External content is recorded and marked as external** ([`plugins.md` §10.2](plugins.md#102-it-behaves-like-music)). It is still listening, and leaving it out would make a user's own history wrong — but it never influences recommendations ([`recommendations.md` §1](recommendations.md#1-what-can-be-recommended)).

---

## 2. Derived Measures

Raw plays are a weak signal on their own; a track started thirty times and skipped thirty times looks identical to one genuinely loved. Listen time separates them.

- **Completion** — listen time against total possible listen time (duration × plays). Near zero means a track is repeatedly skipped; near one means it is heard through; above one means parts are replayed.
- **Skip behaviour** follows directly, with no separate signal needed: a play that ended early and accumulated little listen time is a skip.
- **These, not play counts, drive recommendations.** Because there is no explicit way to say "I like this" ([§7](#7-no-explicit-signals-in-v1)), completion is the primary evidence of enjoyment, and treating a high play count as approval without it would actively mislead.
- **Derived measures are recomputed from the record**, never stored as a separate truth that could drift from it.

---

## 3. Listening History

- **A chronological record of every play**, most recent first, across every device.
- **Browsable and searchable** — by date range, artist, album, track, or library.
- **Each entry says where it came from**, so a user can see what they were listening to and why it played.
- **Distinct from a queue's History** ([`queue.md` §2](queue.md#2-the-queue-panel)), which covers only the current session. This is the permanent record.

---

## 4. Statistics

Available over **7 days, 30 days, 90 days, a year, and all time**, per library and across all of them:

- **Top tracks, albums, artists, and genres** ([`tags.md`](tags.md)).
- **Total listening time**, and whether it is rising or falling against the previous period.
- **Time-of-day and day-of-week patterns** — when a user actually listens.
- **Rediscovery** — when something was first heard, and what has not been played in a long time.

Rules:

- **Every statistic is reachable from the entity it concerns.** An artist page shows how much that artist has been played, not just a global leaderboard.
- **Ranking accounts for completion** ([§2](#2-derived-measures)), so a skipped track does not out-rank a loved one on play count alone.
- **Statistics are computed, never estimated.** A number shown to a user is the real number.

---

## 5. Period Summaries

A recap of a period — a year, or any range the user picks — presented as something worth reading rather than a table.

- **Top artists, albums, tracks, and genres** for the period.
- **Favourite release years and decades**, which reveal whether someone spent a year in the 70s or on new releases.
- **How the period differed from the one before it** — what arrived, what fell away, whether listening grew.
- **Standout moments** — the most-repeated track, the biggest listening day, the artist that came from nowhere.

Summaries are **generated from the record on demand**, so a user can ask for any period rather than waiting for an annual event.

---

## 6. Offline and Reconciliation

- **Plays are recorded at the time they happened**, not the time they synced. A week offline lands on the right days ([`offline.md` §8](offline.md#8-reconnection)).
- **Listen time survives the same way**, so offline listening is not analytically second-class.
- **Nothing is counted twice.** Reconnecting repeatedly, or on several devices, never inflates a count.
- **Recording never depends on a server.** A device with no connection records locally and reconciles later.

---

## 7. No Explicit Signals in v1

**There is no favouriting, liking, or rating, and no hiding.** Enjoyment is inferred from behaviour — what a user plays, finishes, repeats, and skips.

This is a real constraint rather than an omission, and it has consequences worth stating plainly:

- **There is no Favourites collection**, and no way to mark a track a user loves but rarely plays.
- **Recommendations rest entirely on implicit signals** ([§2](#2-derived-measures)), which makes completion data more important than it would otherwise be.
- **Nothing can be suppressed.** A track a user dislikes keeps appearing until their listening behaviour says otherwise.

Adding an explicit signal later must not invalidate what was recorded before it — the implicit record stands on its own.

---

## 8. Durability

- **History survives its tracks.** When a file is purged from the library, past plays remain readable, showing the correct title and artist, marked unavailable ([`scanning.md` §8.1](scanning.md#81-purging-never-costs-history)).
- **Statistics never shift retroactively.** Cleaning up missing files does not change what a user listened to last year.
- **Retagging does not rewrite history.** Identity follows content ([`scanning.md` §6](scanning.md#6-track-identity)), so a corrected tag updates how a past play is displayed without altering that it happened.

---

## 9. Privacy and Control

- **Private between users** ([`users.md` §7](users.md#7-privacy--personal-data)). Admins can view any account's history and statistics, and can never modify them ([`users.md` §8](users.md#8-admin-capabilities--visibility)).
- **Clearable individually or entirely**, and clearing is permanent.
- **Deleted means deleted.** Removing history removes its influence on statistics and recommendations. A cleared play must not resurface through an aggregate derived from it.
- **Exportable** in a portable, documented format, by the user or by an admin on their behalf ([`users.md` §9](users.md#9-account-lifecycle)).

---

## 10. External Services

**Scrobbling to Last.fm, ListenBrainz, and similar services is a plugin concern**, not a core one.

**Plays are exposed with enough detail to scrobble accurately** — track, artist, album, timestamp, and listen time — since a plugin that has to guess at these produces bad data on somebody else's service.
