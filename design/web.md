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
| `src/lib/queueing.svelte.ts` | Adding to the queue from every menu and drag, and the toast that says what was done (*The queue panel* below) |
| `src/lib/core-wasm/`       | The shared core built to WebAssembly by `pnpm wasm` (gitignored); needs the `wasm-bindgen` CLI at the version in `scripts/wasm.sh` |
| `src/lib/queue.ts`         | What a start from a view sends (`PlayFrom`), and when Previous restarts rather than steps back                                                          |
| `src/lib/sorting.ts`       | What each track list sorts by, and each sort's name in the API (`listTracks`' names, and `addedToPlaylist`) |
| `src/lib/track-sort.ts`    | Sorting a list the page holds by the core's comparator through `core-wasm`, the order the server reproduces for a recipe (`design/general.md` §3). Only the pages that sort locally import it, so no other page waits for the core; they and the queue worker download the one WebAssembly file between them (`design/general.md` §7.1) |
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
- **The queue** has no endpoints (`/me/queue*`, `/me/queues*`, the tracks-by-IDs lookup), so the queue panel and the player work only against the real server, or in the end-to-end tests, which stub them.
- **Accounts** don't exist: the mock reports setup as done, everyone is one user, and logging in
  accepts any password.
- **Plugins** can be installed, granted, and enabled, but never run: **Run now** needs the server,
  which hosts them (`crates/plugins`).

## Not built yet

These are client work still to do: virtualized lists beyond the queue panel's (the other long
lists page in as you scroll), loudness normalization, downloads and playing offline, and account preferences. The offline catalog and the installable app are designed in *Catalog worker* and *The installable app* below and are not built yet. Grid/list view and
theme are per-browser for now.

## Catalog worker

**Planned; none of this is built yet.** The device keeps a copy of every library it can reach, in SQLite on the browser's private file system, so pages, search and queue building work offline ([ADR 0008](../adr/0008-sqlite-device-catalog.md), `design/general.md` §3.1). `src/lib/api/client.ts` stays the one module pages call: it sends catalog reads to the worker, which answers with the JSON the server would send.

- **One owner tab.** The browser's file handle for the database belongs to one dedicated worker and is exclusive, so each tab asks for one Web Lock, and the tab that holds it runs the catalog worker. The other tabs send their requests to it over a `BroadcastChannel` and get answers back, and the queue worker reaches the catalog the same way. The database is opened only after the lock is held and is paused before the lock is released, so the next owner can resume it. When the owner closes, the browser hands the lock to another tab.
- **A first sync is a file.** A library with no copy on the device streams the server's snapshot into a temporary database, checking its SHA-256 against the snapshot's ETag as it is written, and swaps it in. Nothing shows until the swap, so a library is never partly there. The feed is then pulled from the snapshot's position.
- **Catching up.** The worker pulls the library's feed and the account's, on start, on every realtime `hello` and on every hint. Pages of changes are saved as they arrive, so a closed tab resumes where it stopped, and are applied together once the head is reached, so a reader never sees half a catch-up. One pull runs per feed at a time, and a hint that arrives during a pull starts another when it ends. A catch-up that would change more than a tenth of a library downloads the snapshot again. Each account pull re-reads the library list, so a library the account gained is downloaded and one it lost is deleted.
- **A copy that cannot be trusted is replaced.** A library the account lost, sign-out, a schema version change, a `410`, a hash that does not match, a corruption error from any query, or storage the browser evicted (iOS does) each delete the copy and start again. The device never runs `quick_check`: it would block the worker for tens of seconds at 500,000 tracks. With no private file system (Safari's private windows) the worker answers nothing and pages read from the server. The app asks the browser to keep its storage and reports quota errors.
- **Where a page reads from** follows the library's state, as in `design/general.md` §7.1. When the device is behind and the server answers too, a different server answer replaces the device's in place, keeping the rows loaded and the scroll position. Sorts the device cannot answer, such as those on personal data the server adds later, always go to the server and say they need it.
- **Live changes.** After applying changes the worker announces that the library changed, and the pages that show it reload in place.
- **Search** goes to the server when it is reachable and to the device otherwise. The worker builds the core's index from the device database the first time search is used and again after changes, no more often than the server does. The device holds the same names the server indexes, so search says nothing about coverage.
- **Queue.** Starting or adding a recipe resolves on the device with the same SQL as the server, hashes with the core and attaches at once. The server's hash is compared when its snapshot arrives and the list is downloaded only if they differ. Offline, a start gets the full list rather than the rows the view held. Lists stay in Cache Storage.
- **Measured at 500,000 tracks** (a laptop, with the worker's CPU throttled about 8 times, and 4 times as the target): the worker answers its first query within a second, and a sorted page or a deep keyset page takes a few milliseconds or less. Reading 500,000 track IDs in a recipe's order took 1.7 to 2.5 s until the device's indexes covered only playable tracks, after which it took 75 ms.

## The installable app

**Planned; not built yet.** The web app gets a manifest with icons, standalone display and the iOS tags, and a SvelteKit service worker with no extra library.

- The service worker keeps the build, per version, so the app opens offline. It takes over the page it was installed from too, so on a first visit the pages not opened yet load offline. A new version installs in the background and takes over as soon as it has installed, with no prompt and no reload of an open page: the page asks the browser to look for one at startup, when it is shown again and when the realtime connection says hello (a server upgrade restarts it), and the next load runs the new build. Open pages keep working, since the worker answers the build's hashed files from the previous version's cache too; it keeps that one older cache and no more, so a page left open across two upgrades must reload to load a new chunk.
- It caches no API response except images. Images are cached as they are seen, in a bounded cache that drops the oldest first, with its size on the account page. Album and artist art draws its ThumbHash placeholder until the picture loads, and when it is not cached offline.
- An indicator shows when the app is offline. A per-device *Work offline* switch stops all network use, with reads local. Surfaces that need the server, such as admin, plugin pages and scan state, say so when offline, and are not hidden.
- Downloads, and choosing between local and stream, are the next part of Stage 10 ([`PLAN.md`](../PLAN.md)).

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
- **An `add` of a `recipe`,** such as an album, applies at once as the number of rows its view
  counts (`edit(edit, length)`), which are `unknown` until the server's list or pages say what
  they are. Its `queueEdit` carries the source as the server registered it, with the length it
  resolved, and replaces the pending add. One sent with no length shows only when that arrives.
- **`queueReplaced`** for a queue other than the one held fetches the new queue; pending edits
  for the old one are dropped as `superseded`. `queuesChanged` is not the worker's: the kept list listens for it (*The queue panel*).
- **Lists.** The store downloads each list the confirmed queue references, `GET
  /me/queue/lists/{hash}`, one at a time in the background, those the item playing and Up Next's
  first 100 rows read first. A list is kept by hash in Cache Storage (`lists.ts`, cache
  `jewelcase-queue-lists`, not the HTTP cache, which iOS evicts quickly) and attached to the core
  when it arrives. A fetch installs a fresh core queue that holds no list, so `#install` shares
  the lists the confirmed queue holds with it (`adoptLists`, which parses nothing), reads only the
  others from storage before the queue is swapped in, and downloads only those it still lacks. When a
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
  list is on its way or not yet confirmed, and `unavailable` when its list is `gone`, when it is
  `failed` and no page that holds the row is being fetched, or when a page says the server cannot
  read it, so the player can skip it in place. Pages,
  `GET /me/queue/rows`, are fetched for the first 100 rows of Up Next and of History and the item
  playing as soon as any lacks a track, and for a requested window; they fill by item, so any
  version's is right. A page is asked for again only when the confirmed version changes (or on
  `hello`), so a window asked for again whenever `revision` changes never loops. What a page gave
  for a source is dropped once its list is attached. No edit waits for a list or a page. The last
  fill of §3, the rows of the view a start came from, is *Starts* below.
- **Starts.** `start()` (the `start` message, `QueueClient.start`) makes the queue the new one at
  once, as a start from a view does: the state is the core's own start (`Queue.started`), the
  source as long as the view's `total` (or its `tracks`), no list, the clicked row current and
  the cursor after it, and the confirmed queue's repeat mode, which the server's start copies
  too. The latest repeat change not yet confirmed is not dropped with the other pending edits:
  it is made again on the new queue and sent after the start, so the device and the server both
  end on the mode last chosen. It goes before every edit and fetch, so `POST /me/queue` is sent
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
  is `unknown`, however the server answers: its list is on its way, after a confirmed start in
  the snapshot that follows and after an add of this device's own in the add's answer, so the
  player waits for it rather than skip it.
- **Connection.** `connect()` spawns the worker and passes it `hello`, `queueEdit` and
  `queueReplaced`. A request that fails, or answers 429 or 5xx, is tried again after a wait.

## Player

`player.svelte.ts` plays the queue's current item through one audio element, and moves through
the queue only by editing it (`requirements/queue.md`, `design/queue.md` §3).

- **A start from a view** (`play(tracks, index, context, from?)`) plays the clicked track, whose
  summary the view holds, before anything is answered, and starts the queue in the background. The
  item is `<key>.<index>`, which the report names. With `from`, a recipe and the view's `total`
  (`PlayFrom`), the request carries the recipe, `start`, `startTrackId`, `total` and the loaded
  rows; without it, the tracks. Album, artist, songs, search and playlist views send a recipe,
  carrying the view's tag filter as `filters` (`tagAll`, `tagAny`) so the server resolves the
  rows shown; a sorted search sends tracks, since no recipe gives its order. A playlist's recipe
  is `{type: 'playlist', playlistId}` with the page's `sort` and `order` together, named as
  `listTracks` names them and date added as `addedToPlaylist` (D41), or neither for the
  playlist's own order (D40); the server resolves the stored
  playlist with the core's comparator, so it equals the rows shown, and never takes a `version`
  from the device. A playlist card sends the bare recipe and queues by the summary's `trackCount`
  without fetching its items. The
  recipe's filter is the one the rows were loaded under (`data.filter`), or on an album and a playlist, which
  filter their own tracks, the filter shown. An album or playlist whose `tagAny` tag has no known type (not
  carried by its tracks, not yet read) would group it differently from the server, so it sends
  the tracks shown until the type is read; so does a playlist with edits not yet confirmed, which
  the server would resolve as last saved. So does a playlist sorted by date added once songs were
  added on the page, an undo's included: the server dates them by its own clock as it saves them,
  and the page keeps this device's dates. A Next pressed before the
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
  changes, once per change. A play records the queue's context. Its `context` and `origin` are the device's, as sent (the server cannot tell the Manual Queue from the context for a play reported after the queue changed), and the web records `manual` for a play of an item from the Manual Queue (the worker's
  `playingManual`, from the core), `context` otherwise.
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
  album, artist and playlist pages and cards (in the order shown), the Songs page, and an
  unsorted search's songs, each under the view's tag filter; the device holds no tracks for these, so the add shows at once as the view's count of
  rows, which wait for the server's list or pages. **What goes as `tracks`:** individual song rows, selections and sorted search rows; these show at once from the tracks sent. **Dropping at
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
