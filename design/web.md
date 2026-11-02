# Jewelcase web client

The responsive web app: Svelte 5 + TypeScript on SvelteKit in SPA mode (`design/general.md` §1, §7.1).

## Running it

The client talks only to the public API (`api/openapi.yaml`). Until the Rust server implements
that API, `vite dev` and `vite preview` answer `/api/v1` with a **mock** (`mock/`) that reads the
database the real scanner writes. So you first scan a music folder with the server, then run the
client.

```sh
# 1. Scan once from the repository root. Leave it running if you want waveforms and loudness: analysis
#    runs in the background and starts about 30 seconds after launch.
JEWELCASE_MUSIC=/path/to/music JEWELCASE_DATA_DIR=./data cargo run -p jewelcase-server

# 2. In another terminal, start the client.
cd web
pnpm install
pnpm dev            # http://localhost:5173
```

The mock reads `$JEWELCASE_DATA_DIR/state/jewelcase.db`, with `JEWELCASE_DATA_DIR` defaulting to
the repository's `data/`. The server defaults to `/data`, the path inside its container, so step 1
sets it. With no database the mock serves an empty server, and the client shows its "No music yet"
state.

## Scripts

| Command                       | What it does                                                   |
| ----------------------------- | -------------------------------------------------------------- |
| `pnpm dev`                    | Dev server with the mock API                                   |
| `pnpm build` / `pnpm preview` | Static production build in `build/`, and a local preview of it |
| `pnpm check`                  | Type-check, including the mock                                 |
| `pnpm lint` / `pnpm format`   | ESLint and oxfmt                                               |
| `pnpm test:unit`              | Vitest, in Node and in a headless browser                      |
| `pnpm test:e2e`               | Playwright, against `preview`, with fixture data               |
| `pnpm gen:api`                | Regenerate `src/lib/api/schema.d.ts` from `api/openapi.yaml`   |

API types are generated, never hand-written. After editing the OpenAPI spec, run `pnpm gen:api`
and commit the result.

## Layout

| Path                       |                                                                                                                                                         |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/lib/api/`             | Generated schema types, the aliases in use, and the typed client. Catalog reads move to the worker later, and this is the one module that changes then. |
| `src/lib/player.svelte.ts` | Playback: one audio element playing the queue's current item, starts from views, and Media Session (*Player* below)                                    |
| `src/lib/queue/`           | The queue on this device: a worker holding it through the shared core, and the page's client of it (`design/queue.md`)                                  |
| `src/lib/queue.ts`         | What a start from a view sends (`PlayFrom`), and when Previous restarts rather than steps back                                                          |
| `src/lib/components/`      | Shell (sidebar, tab bar, player), collection and track views, waveform                                                                                  |
| `src/routes/`              | One route per view: `/albums`, `/albums/[id]`, `/artists/…`, `/playlists/…`, `/songs`, `/search`, `/account`; `/setup` and `/login` without the shell   |
| `mock/`                    | The dev-only mock API. Never bundled into the client.                                                                                                   |

## What the mock does not do

It is a stand-in for the server, so it has these limits. None of them are client behaviour.

- **Search** is a case-insensitive substring match, not the real typo-tolerant search.
- **Audio** is always the original file with no transcoding, so formats the browser can't decode
  (AIFF in Chrome, for example) are skipped.
- **Playlists** are kept in memory, so they reset when the dev server restarts, and a few are
  generated from the library to start with. They can be created, edited, and deleted, but have
  no custom artwork, mixes to save, or external tracks.
- **Personal data** (play counts, listening history) doesn't exist, so sorts that need it fall back
  to album or name order.
- Only the read endpoints the client uses are implemented; any other write answers `405`.
- **Accounts** don't exist: the mock reports setup as done, everyone is one user, and logging in
  accepts any password.
- **Plugins** can be installed, granted, and enabled, but never run: **Run now** needs the server,
  which hosts them (`crates/plugins`).

## Not built yet

These are client work still to do: virtualized lists beyond the queue panel's (the other long
lists page in as you scroll), loudness normalization, offline, and account preferences. Grid/list view and
theme are per-browser for now.

## Queue worker

`src/lib/queue/` holds the account's queue in a web worker, so loading and editing a queue of any
size stays off the page's thread (`design/general.md` §7.1). `worker.ts` is a shell around
`QueueStore` (`store.ts`, plain TypeScript, so it also runs in tests); `client.svelte.ts` is the
page's side, and `protocol.ts` is what they say. The store runs the shared core, built to
WebAssembly by `pnpm wasm`, and holds no rule about the queue's order. Its state holds no tracks:
the core knows each source's length and, once attached, its list, which the store fills in from
the server (*Lists and rows* below, `design/queue.md` §3).

- **Snapshot.** On every realtime `hello` the store fetches `GET /me/queue`, naming the `ETag` it
  holds, `"<run>.<queueId>.<version>"`. The server answers `304` when nothing changed, so a
  reconnect downloads and rebuilds nothing, and the page hears nothing. Otherwise the state loads
  into the core with `Queue.fromJson`. `404` is no queue.
- **Two queues.** The *confirmed* queue is the snapshot with every `queueEdit` applied in version
  order. The *view*, which the page shows, is a `copy()` of it with this device's *pending* edits
  replayed on top, so the copy shares attached lists. With nothing pending the view is the
  confirmed queue itself.
- **Edits.** `edit()` applies at once to the view, then sends `POST /me/queue/edits` in the
  background, one request at a time and in order, before any fetch. Each carries a device-minted
  `editId`, resent unchanged if the answer is lost, so the server applies it once (D14). The edit
  carries no base version. A `queueEdit` one past the confirmed version applies to it; if it is
  ours, by `editId`, the pending edit is done, so it is never applied twice. A gap, another
  queue's edit or an edit the core refuses means the copy cannot be trusted, and the queue is
  fetched again.
- **Edits that do not stay.** The store reports each, with a `Refusal` carrying the code and the
  edit, to the page (`client.refusal`, shown as *The queue panel* says): the core refused it here
  (`anchor_gone`, `invalid`); the server refused it (`anchor_gone`, `play_gone`, 422 and the
  like), and the view is rebuilt without it; a `queueEdit` heard first took what a pending edit
  needed (the core's code on the replay); a new queue replaced it (`superseded`); or the server
  answered it but, after a restart, no longer has it (`lost`), which is told by its version
  being past the snapshot's. A lost edit is not sent again, since that could apply it twice.
- **An `add` of a `recipe`,** such as an album, has no length here, so it is only sent. It shows
  when its `queueEdit`, carrying the source as the server registered it, arrives, and edits made
  after it replay on top.
- **`queueReplaced`** for a queue other than the one held fetches the new queue; pending edits
  for the old one are dropped as `superseded`. `queuesChanged` is not the worker's: the kept list listens for it (*The queue panel*).
- **Lists.** The store downloads each list the confirmed queue references, `GET
  /me/queue/lists/{hash}`, one at a time in the background, those the item playing and Up Next's
  first 100 rows read first. A list is kept by hash in Cache Storage (`lists.ts`, cache
  `jewelcase-queue-lists`, not the HTTP cache, which iOS evicts quickly) and attached to the core
  when it arrives. A fetch installs a fresh core queue that holds no list, so `#install` attaches
  the stored ones again before the queue is swapped in, and downloads only those it lacks. When a
  change leaves the queue no longer referencing a hash, as clean-up drops a source, the list is
  released: deleted from storage. Loading a snapshot also deletes every stored list it does not
  reference. A `404` marks the list `gone` for as long as the queue references it; a failed
  request, `failed`, is tried again on the next `hello`. Where Cache Storage is missing or throws,
  as in a private window, lists stay in memory instead and pages cover the gap. Every Cache
  Storage call is bounded by a 2 s timeout; past it Cache Storage is given up for the session, so
  a cache that never answers cannot stall the queue.
- **Own adds.** An add of `tracks` shows its tracks at once: with no list and no page for a row,
  `rows()` reads the pending add's `tracks` by the row's source and position, which is right
  through a copy or the shuffle, since the server's list is those tracks. It needs no hash. When
  the `queueEdit` names the `list`, the tracks are kept under that hash and attached to the
  confirmed queue, so they are never downloaded back and no page is fetched.
- **Rows.** `rows()` (the `rows` message, `QueueClient.rows`) answers a window of `current`, `upNext`
  or `history` with each row's `item`, `libraryId` and `track`, filled from the attached list,
  else from a page (`rows.ts`), else from a pending add's own tracks, else not at all. A row without a track is `unknown` while its
  list is on its way or not yet confirmed, and `unavailable` when its list is `gone` or `failed`, or
  a page says the server cannot read it, so the player can skip it in place. Pages,
  `GET /me/queue/rows`, are fetched for the first 100 rows of Up Next and of History and the item
  playing as soon as any lacks a track, and for a requested window; they fill by item, so any
  version's is right. A page is asked for again only when the confirmed version changes (or on
  `hello`), so a window asked for again whenever `revision` changes never loops. What a page gave
  for a source is dropped once its list is attached. No edit waits for a list or a page. The last
  fill of §3, the rows of the view a start came from, is *Starts* below.
- **Starts.** `start()` (the `start` message, `QueueClient.start`) makes the queue the new one at
  once, as a start from a view does: the state is the core's own start (`Queue.started`), the
  source as long as the view's `total` (or its `tracks`), no list, the clicked row current and
  the cursor after it. It goes before every edit and fetch, so `POST /me/queue` is sent
  first, again with the same body if the answer is lost, and edits made meanwhile name the new
  `queueId`. A start of `tracks` is answered with `{list, length}`: the tracks are attached under
  that hash and kept, so nothing is downloaded back. A start of a `recipe` then fetches the
  snapshot, with no `If-None-Match`, since the server's list and start may differ from the view's.
  A fetch that began before a start, or whose install was waiting on stored lists, is never
  installed over it. If the server's list is not as long as the view's, or its snapshot makes
  another item current, the view's rows are dropped, so the row waits for a page. A start the server refuses (4xx)
  is taken back: the edits made since are dropped as `superseded`, the failure is reported with
  no edit, and the server's queue is fetched.
- **The view's rows.** A recipe start holds the rows its view loaded, by position. While the
  queue is unshuffled, or for the item playing, they fill the rows read straight from the source,
  after the list and a page, so a page the server gives replaces them if the library changed.
  They are dropped once the list is attached or clean-up drops the source. A row nothing can fill
  is `unknown`, however the server answers: it waits for the snapshot that follows a confirmed start,
  and the player waits with it rather than skip it.
- **Connection.** `connect()` spawns the worker and passes it `hello`, `queueEdit` and
  `queueReplaced`. A request that fails, or answers 429 or 5xx, is tried again after a wait.

## Player

`player.svelte.ts` plays the queue's current item through one audio element, and moves through
the queue only by editing it (`requirements/queue.md`, `design/queue.md` §3).

- **A start from a view** (`play(tracks, index, context, from?)`) plays the clicked track, whose
  summary the view holds, before anything is answered, and starts the queue in the background. The
  item is `<key>.<index>`, which the report names. With `from`, a recipe and the view's `total`
  (`PlayFrom`), the request carries the recipe, `start`, `startTrackId`, `total` and the loaded
  rows; without it, the tracks. Album, artist, songs and search views send a recipe; a sorted
  search and playlists send tracks, since no recipe gives their order. A Next pressed before the
  worker has the queue is carried out once it has.
- **Following.** A device holding playback plays whatever the queue's current item becomes,
  whoever moved it. The item's row is read from the worker. `unknown`, a row still being learned,
  waits, and the player looks again with each change to the queue. `unavailable`, or a track
  that is missing, out of reach (403, 404) or fails as media, is skipped in place: one more
  `next`, or `previous` when going back, applied at once. Going back past History's start goes on
  forward. A failure to reach the server (a network error, 5xx, 429) is never a reason to skip:
  the player waits paused and asks again every 3 seconds. A run of skips stops when it
  comes round to an item it already passed over (reset when audio plays): playback stops and says
  that nothing in the queue can play, under Repeat All too. A pause cancels a track waiting to be
  asked for again. If the server's queue makes the track just started
  another item, the same track goes on under the new item. A start the server refuses leaves the
  player paused on the chosen track with the refusal shown, while the old queue comes back.
- **Controls.** Next and a track's end send `next`; Previous restarts a track played more than 3
  seconds, or with an empty History, and otherwise sends `previous` with a fresh key; Repeat One
  plays the item again here, and Repeat All is the queue's own. `jump(item)`, shuffle (with a
  device-chosen `u32` seed) and repeat are edits too. The next row's track is fetched ahead.
- **Reports** name only the `itemId`, which the server refuses unless the account's active queue
  holds it. So a report waits until the server has the item: a start's takeover for the start's
  answer, a Previous's copy for its edit's confirmation (the queue worker says which item the
  server's queue has playing). A report the server refused is sent again when the queue next
  changes, once per change. A play records the queue's context.
- **Restores.** `restore()` (the `restore` message, `QueueClient.restore`) sends `POST
  /me/queues/{id}/restore`, which is not retried, since a second restore of the same queue is
  refused as active. Nothing shows until the server answers, since only it holds the queue; its
  snapshot is installed as a fetch's is (stored lists attached, edits for the replaced queue
  dropped as `superseded`, edits heard meanwhile replayed) and the answer carries `position`, or
  why it was refused (`queue_active`, `not_found`, or `unreachable` with the server out of reach,
  after which the queue is fetched again).
- **Taking over** a track another device plays needs the state's `itemId` to be the queue's
  current item, with its row known: playback moves without rebuilding the queue. It fails, and
  starts nothing, while the queue here is not that far or the item is of another library.

## The queue panel

Opened from the player bar, over the main area like Lyrics and Devices. It shows and edits the
account's queue, so it works the same whichever device plays: every edit goes through the queue
client, applied here at once and sent on, and the device holding playback follows the queue. It
has three tabs, Up Next, History and Kept, an accessible tab list (arrows, Home and End move between the
tabs, and the selected tab's panel is labelled by it); each tab draws its own list, so nothing but
the tab's name carries from one to the other.

- **Up Next** is the Manual Queue under its own heading (a hint stands in when it is empty), then
  the context's items under theirs. **Shuffle and Repeat** (Off, All, One) sit beside the heading,
  since the player bar hides them at phone width; they go through the player (`setShuffle`,
  `setRepeat`), so a shuffle sends a random 32-bit seed.
- **Rows are one fixed height, so the list is virtualized by arithmetic** (`queue/layout.ts`,
  `QueueList.svelte`): the rows to draw come from the scroll position, and the worker is asked for
  those rows only (`rows('upNext', offset, limit)`, the Manual Queue's first). Each is drawn as
  soon as the worker names its item, and shows its track once the batch lookup by IDs has it
  (`queue/summaries.ts`, at most 100 IDs per library per request, never none). A `loading` row is
  one the worker is still learning (it is asked again when the queue changes); an `unavailable`
  one, or a track out of reach or missing, shows "Unavailable". A 500,000-item Up Next draws about
  twenty rows. The list is drawn at most 10 million pixels tall, since Firefox ignores a taller
  height, and past that the scroll position is scaled into the whole list.
- **Editing.** A grab handle drags one row, or the selected rows together, anywhere in Up Next,
  across the Manual Queue's end too (`move`, with an anchor from `anchorAt`); the handle takes no
  scroll from a finger, and pressing it without moving selects the row. From the keyboard, Space on
  a title selects, Alt+↑/↓ moves the selection (or the row) a place, Delete removes. A row's ✕
  removes it, or the selection it is in. Presses made before the list has caught up with the last
  edit wait for it, so holding a key moves one place per press. Clicking a row jumps to it
  (`player.jump`), and everything ahead of it goes to History.
- **History** is the same virtualized list, newest first, read as `rows('history', offset, limit)`
  (each row carries its play number) and shown through the same by-IDs lookup. A row is named
  `<play>:<item>`, since Repeat All can play an item more than once, and every History edit names
  `{play, item}`. Clicking a title, or Enter on it, goes back to that play (`player.rewind`, a
  `rewind` edit with a key minted here, the device taking playback as for a jump); the ✕, or
  Delete, removes it (`removePlays`); the plus puts it at the front of Up Next (`requeue`, with its
  own key) and takes it out of History. Rows do not move, so where Up Next has its grab handle
  History has a check button that selects, and Space on a title selects too; the ✕ removes the
  whole selection. A row that is still being learned or cannot be read cannot be rewound to or
  re-queued, and can be removed. An empty History says nothing has played.
- **Kept** is the third tab, so kept queues (`requirements/queue.md` §6) are reached the same way
  at every width, by touch, mouse and keyboard, beside the queue they were once the active one of
  (`KeptQueues.svelte`). The list is the server's: `GET /me/queues`, paged with its cursor by a
  Show more button, fetched when the tab opens, on `queuesChanged` and on `hello`, and again as
  far as was shown, so a change does not send the person back to the top. Each entry names the
  song it was playing (the by-IDs lookup), its context (an album, artist or playlist by name,
  looked up in the library of the song; otherwise by kind), its Up Next and History counts and
  when it was last active. Pressing the entry restores it (`player.restore`): the player waits
  for the server's answer (`#starting`, so the queue playing is not followed meanwhile), then
  takes playback over here as a start does, plays the current item from `position` (`#resumeAt`,
  used when the item loads) and reports it as a start, which it can since the queue it names is
  now the server's. The panel then shows Up Next. The trash button deletes with one press, and
  `queue_active` is explained: the active queue is replaced by a start. A failed restore is
  shown and nothing changes. **Offline** the list and a restore need the server (`design/queue.md`
  §7): the tab says so and offers Try again, which `hello` also does when the connection returns.
- **Adding to the queue** (`queueing.svelte.ts`). Every ⋯ menu (song rows and the selection bar,
  album, artist and playlist cards and detail headers, the Songs page and the search Songs
  header) opens Queue next, Queue last and Add to Up Next…, above Add to playlist (which a view
  with no entity, such as all songs, lacks). They are an `add` at `manualStart` / `manualEnd`, or
  at a point chosen in Up Next, and a toast says what was done at once. **What goes as a recipe:**
  album and artist pages and cards (in the order shown), the Songs page, and an unsorted search's
  songs; the device holds no tracks for these, so the store keeps the add pending and its rows
  wait for the server's `queueEdit`. **What goes as `tracks`:** songs, selections, playlists (in
  the order shown) and sorted search rows; these show at once from the tracks sent. **Dropping at
  a point:** with a mouse (`pointer: fine`) song rows, the selection and cards drag from a view
  onto Up Next, which draws the drop line and scrolls near its edges; the gap becomes an anchor
  with `anchorAt`. Touch and keyboard use **choose where**: Add to Up Next… opens the panel on Up
  Next with a bar (Add to the end, Cancel), an Add-here button and the title of each row, and
  Add first / Add last on the Manual Queue's headings; Escape or Cancel gives it up, and closing
  the panel clears it. **With no queue** these say "Start something playing first, then queue
  more." and send nothing, since the core has no empty queue to add to (D25).
- **A change that did not stay is explained** in the panel from the client's `refusal` (`anchor_gone`,
  `play_gone`, `superseded`, `lost`, ...), until dismissed. A rewind to a play another device
  removed first (`play_gone`) is dropped from the view, as every refused edit is, and the panel
  says that song is no longer in History.
