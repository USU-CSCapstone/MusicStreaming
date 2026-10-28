# Queue Design

## Overview
How the server keeps each account's queue, as built so far: the model, who names what, where it is kept, and what goes over the wire. It builds on [`requirements/queue.md`](../requirements/queue.md) and the shared core ([`general.md` §3](general.md#3-shared-core)). Edits, realtime messages and the player's use of the queue are not built yet.

**The one idea: store the inputs, send the edits, and compute the order in the core everywhere.** The queue's rules live in `crates/core` (`queue.rs`), which the server uses natively and the web client through WebAssembly ([`requirements/queue.md` §7](../requirements/queue.md#7-across-devices), [`requirements/general.md` §3.5](../requirements/general.md#35-offline-parity)). The server holds no rule of its own about order.

---

## 1. Model

- **Sources are frozen.** Starting a queue resolves its context once into a source: `{key, libraryId, tracks[]}`. It is never rewritten, so a scan cannot reshape a running queue, devices on different catalog versions cannot disagree about it, and it restores exactly as left ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)). A track that goes missing stays in place and is skipped ([§8](../requirements/queue.md#8-resilience)).
- **An item is a position in a source**, named `<sourceKey>.<index>`. The same track twice is two items, so playback, shuffle and unshuffle never act on the wrong copy ([§8](../requirements/queue.md#8-resilience)).
- **The state is lists of item numbers**: the Manual Queue, the context's order with a cursor (items before it have been passed), the current item, History, the shuffle seed and the order it replaced, and the repeat mode. An item number counts the tracks of the sources laid end to end, so a 500,000-track order is 4 bytes an item, and a shuffle changes only that order. Measured at 500,000 tracks: the order lists are about 2 MB (4 MB while shuffled), the track IDs the sources hold about 27 to 33 MB of memory in all, a snapshot about 18 MB of JSON, and the source's database row about 11 MB.
- **Shuffle is seeded.** The device chooses the seed, so a shuffle works offline, and the same seed gives the same order on every device and native or WebAssembly ([§4](../requirements/queue.md#4-shuffle)).

## 2. Identity

- **The device that starts a queue chooses its `queueId` and its source's `key`**, both UUIDs, so it can apply the start at once and confirm later ([`requirements/general.md` §3.4](../requirements/general.md#34-optimistic-by-default)). Any device can choose any ID, so a `queueId` names a queue only among its account's own, as a `playId` does ([`database.md` §1](database.md#1-conventions)); every read and write is by account.
- **Keys starting with `~` are the core's.** It mints them for the copies a Repeat All restart makes and for a carried-over Manual Queue, so a device's key never collides with one.
- **A start sent again, by `queueId`, changes nothing**, so a device may retry it without losing what played since.

## 3. Starting

A start replaces the account's queue: kept queues are not built ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)). It carries the Manual Queue over, the default there, by copying those tracks into new `~` sources in the new queue, so their item IDs change with the queue ID. The version restarts at 0.

**Every track must be in the named library, and the caller must reach the library** ([`requirements/general.md` §3.6](../requirements/general.md#36-libraries-are-the-isolation-boundary)). One query counts the distinct IDs that are tracks of that library, and the start is refused unless it counts them all. An ID that is malformed, unknown, in another library, or in a library the caller does not reach is refused with the same `422`, so no one learns what a library they cannot reach holds ([`requirements/users.md` §10](../requirements/users.md#10-access-semantics)). A track that is missing, as the scanner marks it, is allowed. `context` is kept as sent, checked as a play's is: an object with a string `type` of at most 4 KiB.

A request is limited to a 2 MiB body, about 90,000 tracks. A device that does not hold the whole list it shows starts from the list's query instead (below).

### From a view's query

A view that holds all its tracks, an album or a playlist, sends them. The others, the library's songs, an artist's tracks and search results, may be far too large to send or even to hold, so the device sends the query its view pages through and the track the user chose, and the server resolves it to a source in the same way ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)).

- **The query is the list's own.** `{type: "tracks", ...}` takes the filters and sort of `listTracks`, read by the same code, so the answer has the order paging that list to its end gives with the same parameters, and the same refusals (`422`). `cursor`, `limit` and `ids` name part of a list and are refused. The server pages the list internally a thousand rows at a time, the same keyset queries a client's pages use, and reads only the IDs.
- **A search is the whole track section.** `{type: "search", q}` takes every track the search ranks, best first. The search endpoint caps a section at `sectionLimit` (50) and its ranking cannot be paged, so a search start is not "what paging gives"; it is the endpoint's `types=tracks` section without the cap, whose first `sectionLimit` tracks are exactly what the device was shown. A track the index knew of that has since been deleted is left out, as a search's results leave it out.
- **The start is a track, not an index.** The list may have changed between what the user saw and the request, so `startTrackId` names the track and the queue starts at its first place. A track that is not in the list is refused with `422`, like a track out of reach: starting at 0 would play something the user did not choose.
- **Reach.** The library must exist and be reached, or the start is refused with the same `422` as a track the caller cannot reach, for an unknown library too. Every track is then in the library by construction, since the query reads only that library's tracks, so nothing is counted as it is for a list of IDs.
- **The query is not stored.** The source is the resolved IDs, frozen like any other, and `context` stays as sent for plays.

## 4. Storage

The queue lives in memory per account while the server runs and is saved in the background, as playback state is (`api/queue/saved.rs`): within about a second of a change, and once more as the server stops. A crash loses at most that second.

| Table | Holds |
|---|---|
| `queues` | One row per account: `queue_id`, `version`, the `context` as sent, and the core's state as JSON, without its sources |
| `queue_sources` | One row per source, numbered in the order they joined: key, library, track IDs as JSON |

- **Sources are written once**, as they join. A save writes only the sources the database lacks and the state, so a shuffle or a move writes the state alone, about 7 MB at most for a shuffled 500,000-track context, and an album's start writes bytes. A 500,000-track source is written once, as about 11 MB. The core gives the state without its sources (`Queue::stored`) and takes them back (`Queue::attach_sources`). A new `queueId` deletes the account's old sources first.
- **One row per item was rejected**: shuffling 500,000 would be 500,000 row updates on a single writer, and the shuffled order would have to be sent to every device.
- **Both tables go with the account** (`ON DELETE CASCADE`). A saved queue is read back at start for active accounts only; one that does not parse is left out and logged.
- **No foreign key to the library.** Once a library is deleted its tracks are simply not found: IDs are random and are looked up with their library, so a later library never inherits them and nothing leaks ([`database.md` §1](database.md#1-conventions)).

## 5. Wire

- **`POST /me/queue`** starts a queue: `{queueId, context, source: {key, libraryId, tracks}, start, shuffle?: {seed}}`, answered `204`. A source of `query` in place of `tracks`, and `startTrackId` in place of `start`, starts from a view's list ([§3](#from-a-views-query)).
- **`GET /me/queue`** answers `{queueId, version, context, state}`, where `state` is the core's JSON, for a device to load into the core. `404` until the account has started a queue: there is no empty queue to show, unlike playback, which can be stopped.
- **An unchanged queue is not sent again.** The `ETag` is `"<run>.<queueId>.<version>"`, and an `If-None-Match` naming it is answered `304`, checked before anything is serialized. `<run>` is drawn as the server starts: a crash can lose edits a device already heard, after which the restored queue can reach the same version with other contents, so a tag from before a restart never matches.
- **The snapshot names IDs, not tracks.** What a device shows for them comes from the track lookup, which serves only tracks the caller reaches, so access taken away later shows rows as unavailable, never details ([`requirements/users.md` §10](../requirements/users.md#10-access-semantics)).
