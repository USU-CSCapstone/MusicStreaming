# Queue Requirements

## Overview
The queue is what plays next. It is more immediate than a playlist — a way to steer the next hour without rearranging anything permanent ([`playlists.md`](playlists.md)).

A queue has three parts:

| | What it holds | Order |
|---|---|---|
| **Manual Queue** | Tracks the user explicitly asked for | Plays first, as added |
| **Context Queue** | The collection they started playback from | Plays once the Manual Queue is empty |
| **History** | What has already played this session | Most recent first |

**A context is anything playable as a run of tracks**: an album, an artist, a playlist, a tag page, a set of search results, a filtered view, a whole library, or a radio station ([`recommendations.md` §4](recommendations.md#4-radio--endless-play)). Every context is recorded against each play ([`analytics.md` §1](analytics.md#1-what-is-recorded)), so a user can always see what they were listening to and why.

Shared behavior follows [`conventions.md`](conventions.md).

---

## 1. Structure

- **The Manual Queue always plays first**, ahead of everything in the context. **Queue Next** inserts at its front, **Queue Last** at its end ([`conventions.md` §2](conventions.md#2-actions)).
- **The Manual Queue is an interruption, not a replacement.** The context does not advance while manual tracks play; when the Manual Queue empties, the context resumes exactly where it was.
- **Played tracks move to History**, manual and context alike.
- **Adding a collection adds its tracks.** Queueing an album, artist, or playlist puts its tracks in individually, each reorderable and removable afterward.
- **A queue can mix owned and external content.** Where a plugin provides an external source, its tracks queue alongside the user's own and play in the same run, marked as external ([`plugins.md` §10.2](plugins.md#102-it-behaves-like-music)).

---

## 2. The Queue Panel

Two tabs, because "what's coming" and "what just played" are different questions:

- **Up Next** — everything still to play, top to bottom in play order. The Manual Queue is **visually separated** from the context, so it is always obvious where Queue Last will land.
- **History** — what has already played this session, most recent first.
- **A track in History can be played again or returned to Up Next**, without disturbing the order of what is upcoming.
- **History belongs to the queue, not the account.** It reflects this queue only, and is distinct from listening history ([`analytics.md`](analytics.md)), which records every play on every device.

---

## 3. Editing

- **Anything can move anywhere.** Drag one track or many — within the Manual Queue, within the context, or across the boundary between them.
- **Remove from any position**, in Up Next or History.
- **Add at a position.** Dropping a track, album, artist, or playlist onto a specific point inserts it there rather than at the end.
- **Reordering and removal are direct** — a visible grab handle for dragging, and single-gesture removal — and work equally by touch, mouse, and keyboard.

---

## 4. Shuffle

- **Shuffle randomizes only the unplayed context.** The Manual Queue keeps its order and still plays first.
- **Unshuffle restores the original order** and resumes from the current track's natural position in it, so turning shuffle off mid-album continues rather than jumping. Keep in mind that the same track can appear multiple times in the same context!
- **Nothing repeats within a session.** Shuffle and endless play never re-select a track that has already played.
- **A shuffled order is fixed once generated.** It does not reshuffle as the user scrolls Up Next, and it is the same order on every device ([§7](#7-across-devices)) and offline ([`offline.md` §6](offline.md#6-what-works-offline)).
- **Repeat All is the one exception, and it does not reshuffle.** Restarting a shuffled queue replays the same order it already generated, exactly as repeating an album replays the same running order ([§5](#5-repeat--endless-play)). A user who wants a different order asks for one by toggling shuffle.

---

## 5. Repeat & Endless Play

Two independent controls, both surviving context changes and device handoff.

- **Repeat** cycles **Off → All → One**. *All* restarts the queue when it ends, in the order it was already playing in — shuffled or not ([§4](#4-shuffle)); *One* repeats the current track until the user changes it.
- **Endless play** decides what happens when a queue ends with repeat off: playback either **stops**, or **continues with similar tracks** ([`recommendations.md`](recommendations.md)). A user who starts an album and walks away can have the music keep going, or not.
- **Endless tracks join the context** and appear in Up Next like anything else, **clearly marked as automatic** so the user always knows why something is playing.

---

## 6. Sessions

- **Starting a new context replaces the current queue** — unless the user has invested something in it.
- **A queue is kept when it has been adjusted**: anything moved, removed, or added, in Up Next or History. An untouched queue is simply replaced, so playing three albums in a row leaves nothing behind to prune.
- **Kept queues are reachable from a dedicated list** and restore exactly as they were left — position, shuffle, repeat, Manual Queue, and History.
- **The Manual Queue's fate is the user's choice.** A preference decides whether manually queued tracks **carry over** into a new context or are **parked** with the session they belong to ([`users.md` §6](users.md#6-profile--preferences)). They carry over by default: the user asked for them explicitly.
- **Kept queues expire after 30 days of inactivity**, or when the user deletes them.

---

## 7. Across Devices

- **One queue, everywhere.** The queue belongs to the account, not a device. What is playing, what is next, position, shuffle, and repeat are identical on every signed-in client.
- **Changes appear immediately.** Reordering on a phone is visible on a desktop without refreshing ([`realtime.md`](realtime.md)).
- **Playback moves between devices** without rebuilding the queue ([`realtime.md` §3](realtime.md#3-shifting-control-and-waking)).

---

## 8. Resilience

- **Size is irrelevant.** Opening, shuffling, or reordering a 500,000-track context is as immediate as a 10-track one ([`performance.md`](performance.md)).
- **Duplicates are positions, not identities.** The same track appearing several times in a context is tracked by position, so playback, shuffle, and unshuffle never act on the wrong copy.
- **Missing and undownloaded tracks are skipped in place** ([`conventions.md` §6](conventions.md#6-availability)). Skipping never renumbers or rebuilds the queue.
- **The queue survives** app closure, restart, and connectivity changes.
