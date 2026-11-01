# Queue Design

## Overview
How an account's queue is represented and edited: the model, the shuffle, the state and every operation on it. It builds on [`requirements/queue.md`](../requirements/queue.md) and the shared core ([`general.md` §3](general.md#3-shared-core)); where it conflicts with [`requirements/`](../requirements/), the requirements win. Storage, the API and the web client are described where they are built; [§7](#7-what-is-not-here-yet) says which.

**The one idea: a queue is a few spans over frozen lists, so its state, storage and network cost grow with edits, not tracks.** An untouched 500,000-track queue is one span. Every rule lives in `crates/core`, which the server uses natively and the web client through WebAssembly, so every device and the server compute the same queue ([`requirements/queue.md` §7](../requirements/queue.md#7-across-devices), [`requirements/general.md` §3.5](../requirements/general.md#35-offline-parity)). No queue is bounded in length ([`requirements/general.md` §3.3](../requirements/general.md#33-the-host-is-the-limiter)), so counts and offsets are `u64`.

---

## 1. Model

A queue is a few **spans** over **views** of **frozen lists**:

- **A frozen list** holds the track IDs a collection resolved to. It never changes, so a scan cannot reshape a running queue, devices on different catalog versions cannot disagree about it, and it restores exactly as left ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)).
- **A view** is a list read as-is, the context's one shuffle, or a copy of some spans.
- **A span** means "positions `a..b` of this view".

The Manual Queue, the context and History are each a short list of spans or runs. Each edit splits at most a couple. Track IDs cross the network in lists, once per device per list, never in queue data.

**An account has one active queue and any number of kept ones** ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)).

- **Every device plays the active queue, and only it is edited.**
- **Starting a new context parks the active queue.** It is kept if it was adjusted ([§5](#5-queue-state)), Manual Queue and all, and deleted otherwise. The new queue starts with an empty Manual Queue: nothing carries over.
- **A kept queue is a stored state.** A restore makes it active again, exactly as left: position, shuffle, repeat, Manual Queue and History.
- **Kept queues expire after 30 days of inactivity**, or when the user deletes them.
- **Its lists belong to the account**, so a kept queue and the active one share any list they both use.

## 2. Lists and sorting

A **list** is `{libraryId, hash, length, recipe?}`.

- **The hash** is SHA-256 over the track IDs in order, as 8-byte little-endian integers, in lowercase hex.
  - Only the server computes it. It never takes one from a device.
  - It names the list everywhere: in queues, on the wire, in storage and in device caches. It must resist collisions, since devices trust it as the list's name.
- **The recipe** is how the list was made. It is not what plays; the hash is. It is kept so a device can rebuild the list from its own catalog once it keeps one ([§7](#7-what-is-not-here-yet)), and so the interface can say what is playing.

  | Recipe | Fields |
  |---|---|
  | `album` | `albumId`, sort |
  | `artist` | `artistId`, `scope`/`artistCredit`, sort |
  | `library` | filters, sort |
  | `search` | `q` |
  | `tracks` | none: the IDs the device sent |

- **A queue's sources** are `{key, libraryId, list: hash, length, recipe?}`. Item `<key>.<p>` is position `p` of that list. A track listed twice is two positions, so two items ([`requirements/queue.md` §8](../requirements/queue.md#8-resilience)).

**The sort is part of the recipe.** Each combination of collection, filters, sort and direction is its own list.

- **Re-sorting a page never changes a playing queue.** Starting from a re-sorted view starts a new queue ([`requirements/conventions.md` §4.1](../requirements/conventions.md#41-track-lists)).
- **Every sort is deterministic** ([`general.md` §3](general.md#3-shared-core)). It compares the core's sort keys, ties end on the track ID and then on the position in the unsorted list, and unknown values sort last.
- **If the server and a device disagree on an order, the hashes differ** and the device downloads the list, so it never plays a wrong order.
- **Sorts a device cannot reproduce always travel as lists:** personal-data sorts (`playCount`, `lastPlayed`, `top`) and the server's search ranking. Freezing them keeps "top tracks" from reshuffling under a playing queue.

## 3. Starting without waiting

**No start is ever waited on, online or offline.** The clicked track plays at once, and Up Next and Next work from every row the device already holds ([`requirements/performance.md`](../requirements/performance.md)).

**The core needs only lengths.**

- A source joins a queue with its length, and its list attaches later.
- Every edit works from lengths and positions: add, remove, move, jump, next, previous, shuffle, unshuffle and rewind. Track IDs are needed only to show or play a row.
- A row whose list has not arrived reads as not yet known, and the device fills it from what it holds. No edit waits for a list.
- **The player waits on a row only while it can learn it.** A row the device cannot learn, offline or because its fetch failed, is unavailable and skipped in place ([`requirements/conventions.md` §6](../requirements/conventions.md#6-availability)), so playback never stalls.

**A device fills a row from the first of these it has:**

1. the list, once attached;
2. a page of rows from the server, which names each row's item;
3. the rows its own view loaded, by list position, for a source it started or added. These fill only rows read straight from the list, never through the shuffle, since the server's list may differ from the device's.

An item's track never changes while the item exists, so a filled row stays right whatever else happens to the queue.

**Whole lists always download in the background**, so the queue plays offline ([`requirements/offline.md` §6](../requirements/offline.md#6-what-works-offline)). A client that cannot hold lists uses pages of rows alone.

## 4. Views, spans and the shuffle

| View | Item at position `p` | Stored as |
|---|---|---|
| `list(key)` | `<key>.<p>` | the source |
| `shuffle` | `base[π(p)]` | `{seed, base: [Span]}`, at most one per queue |
| `copy(key)` | position `p` across its spans, under the new identity `<key>.<p>` | `{key, of: [Span]}` |

**Spans:**

- **A span** is `{view, start, len}`, and is never empty. A span list keeps running lengths, so an index lookup is a search, not a walk.
- **A span names its view by key**, a source's or a copy's, or names the shuffle. So nothing is renumbered when a view is dropped.
- **Counts, offsets and running lengths are `u64`.** In JSON they stay plain numbers, exact to 2⁵³; wasm-bindgen hands them to JS as `f64`.

**Only the shuffled context reads through the shuffle.** Everything else records shuffled rows as the list positions they play: History's plays, rows moved into the Manual Queue, and copies. So at most one shuffle exists, and unshuffle drops it.

**Copies:**

- **One edit makes at most one copy**, under one device-minted key. A copy per run would make the device guess how many runs the server will find, and another device's edit can change that before the server applies this one.
- **Copies are flattened when made.** A span that points at another copy is replaced by the spans it covers, so a copy only points at sources.
- **Sources stay separate from copies.** An optimistic source has no hash until the server confirms it, so a copy reaches its list through the source's key.

**π is a seeded swap-or-not permutation** (Hoang, Morris and Rogaway, 2012). It is computed on demand and never built.

- **Rounds:** R = 6⌈log₂ m⌉ + 6: about 30 for a typical album, about 120 at 500,000.
- **Each round** takes a pivot `Kᵢ` in `0..m`, and position `x` pairs with `x' = (Kᵢ − x) mod m`. A coin flip on `max(x, x')`, keyed by `(seed, i)`, decides whether the pair swaps.
- **Keys and coins** come from SplitMix64, about 15 lines in the core.
- **π⁻¹** runs the same rounds in reverse. Either direction is O(R), well under a microsecond, with no cycle walking.
- **Memory is O(1) at any m.** The shuffled order is never built: with no bound on a queue, one edit could allocate gigabytes.
- **`(seed, m, R)` fix the permutation.** R is part of the saved format, and golden vectors pin native and WebAssembly parity ([`general.md` §3](general.md#3-shared-core)). The seed is a `u32`, chosen by the device so a shuffle works offline.

## 5. Queue state

```text
QueueState {
  sources:  [ { key, libraryId, list: hash, length, recipe? } ]
  copies:   [ { key, of: [Span] } ]            // flattened
  manual:   [Span]                             // the Manual Queue, in play order
  order:    [Span], cursor                     // the context, passed part included
  current:  ItemId?
  history:  [ { first: playNo, span: Span } ]  // play runs, oldest first, newest 1,000 kept
  nextPlay: u64
  shuffle:  { seed, base: [Span] }?            // Up Next reads through it while shuffled
  repeat:   off | all | one
  adjusted: bool                               // decides whether a start keeps or deletes it
}
```

- **`adjusted`** is set by any add, remove or move, in Up Next or History, and is never cleared. Every device and the server set it alike.
  - Playback controls (Next, Previous, rewind, jumps), shuffle, repeat and endless play's additions leave it alone, so playing three albums in a row leaves nothing behind ([`requirements/queue.md` §6](../requirements/queue.md#6-sessions)).
  - Queueing anything sets it, so a queue the user queued into is always kept.
- **Items** are what is queued: the Manual Queue, the context and current. An item's ID never changes while it is queued: through moves, shuffle, unshuffle and Repeat All.
- **Invariant: every item is in Up Next (`manual`, and `order` past the cursor) at most once, and in `order` at most once.** Edits, anchors and jumps name Up Next items, so a second place there would make them ambiguous.
  - **One item can hold two places.** A context track that Previous put back in the Manual Queue keeps its passed place in `order`, so Repeat All still plays it. No field records this: the item's ID is in `manual` and in `order` before the cursor.
  - Moving it within the Manual Queue keeps the passed place. Moving it into the context, or removing it, drops that place too, so a removed track is gone from later laps. Dropping a passed place moves the cursor back one.
  - The core's tests check the invariant after every edit.
- **Clean-up is eager, and in the core.** A source or copy nothing refers to is dropped in the same edit that let it go, with no stubs and no renumbering. "Refers to" means `current`, or a span in `manual`, `order`, the shuffle's base, a copy, or a History run. Every device and the server drop identically, and an edit naming an item of a dropped source is ignored, like any item that is gone.
- **Plays** are History's records.
  - Play #n, "item X played", is recorded when X stops being current: Next, track end, or a jump. Current is never a play.
  - `nextPlay` only counts up. Previous, rewind and removal leave gaps, so a number is never reused.
  - A **run** is consecutive plays of consecutive positions of one view: an album played straight through is one run. Plays are recorded past the shuffle, so each shuffled play is a run of its own.
  - An item may be queued while a past play of it is in History. That is why Repeat All needs no copies.
- **History keeps its newest 1,000 runs** (`HISTORY_RUNS`, a core constant, so every device trims exactly as the server does). That is at most about 100 KB of JSON: 60+ hours of shuffled listening, far more of albums.
- **History edits name `{play, item}` and act only if both match.** A device's play numbers are guesses until confirmed: replaying pending edits after another device's edit can give the same Next a different number. On a mismatch, remove and re-queue are ignored, and rewind fails with `play_gone`, so the device reverts and says so.

## 6. Operations

n items in the queue; s spans in the list edited; B spans in the shuffle's base; r History runs; k items named.

| Operation | What happens | Time |
|---|---|---|
| **Start** | `order: [list[0..len)]` with an empty Manual Queue. The queue it replaces is parked ([§1](#1-model)) | O(1) |
| **Add a collection** | Splice one span in at the anchor | O(s) |
| **Remove k items** | Split the spans holding them | O(k·s) |
| **Move k items** | Remove, then splice in at the anchor. Shuffled rows moved into the Manual Queue go in as list positions, one span per consecutive stretch | O(k·s) |
| **Next / track ends** | The Manual Queue's front becomes current if it has one, and otherwise the cursor moves +1. Append a play (extend the last run or start one) and trim History | O(1) amortised |
| **Previous** | See below | O(1); O(s) to find X |
| **Jump forward over k** | Everything ahead of the target becomes plays: Manual Queue items leave it (a two-place track keeps its passed place), and the cursor moves past context items. One run per span passed, or per item in a shuffle. Only the last 1,000 runs are computed | O(spans passed), or O(min(k, 1,000)) shuffled |
| **Shuffle** | Freeze the unplayed spans as the base. Up Next becomes `shuffle[0..m)` | O(B) |
| **Unshuffle** | See below | O(B + edits while shuffled) |
| **Repeat All restart** | `cursor = 0` over the same spans. No copy | O(1) |
| **Read Up Next row i** | Prefix-sum search to the span, then the view: a list reads directly, a shuffle runs π then the base walk, a copy reads its spans | O(log s), plus O(log B) and π shuffled |
| **Find an item by ID** | Key to source or copy by hash map, then the Up Next span holding that position. In a shuffle: the base position, then π⁻¹ | O(s); O(s + B) shuffled |
| **Remove a play** | Split its run | O(r) |
| **Re-queue a play** | A copy of that position goes to the front of Up Next, and the play leaves History | O(log r + s) |
| **Rewind (click play #k)** | See below | O(r + s) |

**Previous leaves Up Next exactly as it was, with the old current at its front.** Pop the last play, of item X.

1. **The old current goes to the front of Up Next.** Back into the context by moving the cursor back one, if it is the context item just passed and the Manual Queue is empty (an album's usual case). Otherwise to the front of the Manual Queue, a context track keeping its passed place ([§5](#5-queue-state)).
2. **Then X becomes current**, unless X is in Up Next, checked after the old current has moved (after a Repeat All restart, an unshuffle, or an earlier Previous). Then a copy of the play, under the edit's `key`, is current, and X stays queued.

Never pull X out of Up Next instead: that drops it from the context and from every later Repeat All lap. Stepping back over missing tracks is a series of `previous` edits.

**Rewind is exactly Previous, repeated until play #k is current, as one edit.** The queue stays the same, #k plays, then everything heard after it in the order heard, then Up Next as it was: hear 1, 2, 3 (queued manually), 4, 5, rewind to 1, and 1, 2, 3, 4, 5 play again ([`requirements/queue.md` §2](../requirements/queue.md#2-the-queue-panel)).

- The core applies it in two phases with the same result as Previous play by play, so rewinding past a long jump is one step. A History run is consecutive positions of one view, not of `order`, so it can mix Manual and context plays or cross a restart; the phases split runs into pieces where needed.
- **Phase 1,** only if the Manual Queue starts empty: move the cursor back over the longest stretch, the old current and then the newest plays, that matches `order` read backwards from the cursor.
- **Phase 2:** the rest go to the front of the Manual Queue in the order heard, in pieces. A piece still in Up Next after phase 1 goes into the one copy under the edit's `key`, flattened. Any other piece is a span of its view, a context piece keeping its passed places.
- Within an album that is just the cursor moving back: no copy, no new spans. At most one copy of at most r + 1 spans is made.

**Unshuffle never just replaces Up Next.** That would leave items played while shuffled in both the passed part and Up Next, and lose base items the shuffle skipped.

1. **Find the pivot:** the current item if it is in the base, and otherwise the latest passed base item. π of its shuffled position gives its base position j, or −1 if there is none.
2. **Rebuild `order` as** the spans passed before the shuffle, then the base in natural order minus items removed or moved out while shuffled, then context additions.
3. **Place the cursor just after base position j.** Base items before it count as passed, as when starting mid-album. Items after it are Up Next again, whether or not they played while shuffled.
4. **Context additions:** one still in Up Next keeps its distance from current. One already passed stays passed, just ahead of the cursor in the order passed, so Repeat All still plays it.
5. **Moves:** moves within the context revert. Moves into the Manual Queue stay, and so do context tracks Previous put there: they are left out of the rebuilt Up Next. Such a two-place track whose base position falls after the pivot keeps a passed place just before the cursor, so Repeat All still plays it.

With no edits while shuffled, the state returns exactly to its pre-shuffle spans.

**Bounded costs.** Scattered edits fragment spans, each edited row adding one or two, so the state grows with what devices send. Nothing amplifies small edits: shuffle keeps one base, Repeat All adds nothing, and Previous and rewind add at most one copy.

## 7. What is not here yet

- **Storage, the API and realtime messages** are in the docs of the steps that build them. The snapshot's shape is a stable public contract: it carries the state and each list's hash and length, never tracks.
- **Kept queues are online-only until the device keeps its own catalog** (Stage 10, [`PLAN.md`](../PLAN.md)). Their lists and restores need the server until devices store kept states, which falls short of [`requirements/offline.md` §6](../requirements/offline.md#6-what-works-offline) until then ([`requirements/general.md` §3.5](../requirements/general.md#35-offline-parity)).
- **Stage 10 also lets a device build a list from stored orders** and check its hash, downloading only on a mismatch. Until then, devices download every list.
- **Playlists** are the recipe `tracks` until a playlist can name `{playlistId, version, sort}`.

---

## 8. Decisions

| Decision | Why |
|---|---|
| **Span lists over views of frozen lists** | State, storage and network grow with edits, not tracks |
| **Lists are named by a server-only SHA-256, one per account** | Devices can trust a hash as a name, and no device can claim a list it did not send |
| **The sort is part of the recipe, and ends on the track ID** | A playing queue never reshuffles, and every device reproduces an order |
| **Swap-or-not shuffle, one at a time, read only by the context** | O(1) memory at any size; everything else records list positions |
| **Unshuffle rebuilds `order` and places the cursor after the pivot** | Turning shuffle off mid-album continues rather than jumping |
| **History is numbered runs, numbers never reused, and edits match `{play, item}`** | A stale edit changes nothing |
| **Rewind is Previous repeated, in one edit** | Going back never loses what was upcoming |
| **The core needs only lengths, and a row waits only while it can be learned** | Every start plays at once, and playback never stalls |
| **Clean-up is eager, with no stubs and no renumbering** | Every device and the server drop identically |
| **No reserved keys: every key is a device's, unique across sources and copies** | Spans find views by key |
| **No queue bound** | The host is the limiter ([`requirements/general.md` §3.3](../requirements/general.md#33-the-host-is-the-limiter)) |
| **Kept queues from the start, and nothing carries over** | A restore returns a queue exactly as left |
