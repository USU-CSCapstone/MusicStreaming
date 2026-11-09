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
| `src/lib/api/`             | Generated schema types, the aliases in use, and the typed client. Catalog reads go through `catalog/read.ts`, which picks the server or the device, so pages never know which. |
| `src/lib/catalog/`         | The device's copy of the catalog: the page's client, the owner tab's election and request router, the worker and its sync (*Catalog worker* below) |
| `src/lib/catalog-wasm/`    | The catalog built to WebAssembly by `pnpm wasm` (gitignored): SQLite on the browser's private file system, the shared queries and the search index |
| `src/service-worker.ts`    | The offline shell and the image cache (*The installable app* below) |
| `src/lib/connectivity.svelte.ts` | Online, offline or working offline, and what the indicator says (*Connectivity* below) |
| `src/lib/live.svelte.ts`   | Pages that reload in place when the device's copy changes |
| `src/lib/player.svelte.ts` | Playback: one audio element playing the queue's current item, starts from views, and Media Session (*Player* below)                                    |
| `src/lib/queue/`           | The queue on this device: a worker holding it through the shared core, and the page's client of it (`design/queue.md`)                                  |
| `src/lib/queueing.svelte.ts` | Adding to the queue from every menu and drag, and the toast that says what was done (*The queue panel* below) |
| `src/lib/core-wasm/`       | The shared core built to WebAssembly by `pnpm wasm` (gitignored), beside the catalog's; needs the `wasm-bindgen` CLI at the version in `scripts/wasm.sh` |
| `src/lib/queue.ts`         | What a start from a view sends (`PlayFrom`), and when Previous restarts rather than steps back                                                          |
| `src/lib/sorting.ts`       | What each track list sorts by, and each sort's name in the API (`listTracks`' names, and `addedToPlaylist`) |
| `src/lib/track-sort.ts`    | Sorting a list the page holds by the core's comparator through `core-wasm`, the order the server reproduces for a recipe (`design/general.md` §3). Only the pages that sort locally import it, so no other page waits for the core; they and the queue worker download the one WebAssembly file between them (`design/general.md` §7.1) |
| `src/lib/components/`      | Shell (sidebar, tab bar, player), collection and track views, waveform                                                                                  |
| `src/routes/`              | One route per view: `/albums`, `/albums/[id]`, `/artists/…`, `/playlists/…`, `/songs`, `/search`, `/account`; `/setup` and `/login` without the shell   |
| `static/`                  | The web manifest and the app's icons                                                                                                                    |
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
- **Feeds and snapshots** don't exist (`/libraries/{id}/changes`, `/libraries/{id}/catalog`, `/me/changes`), so against the mock a device never gets a copy of the catalog and every read goes to the mock.
- **Accounts** don't exist: the mock reports setup as done, everyone is one user, and logging in
  accepts any password.
- **Plugins** can be installed, granted, and enabled, but never run: **Run now** needs the server,
  which hosts them (`crates/plugins`).

## Not built yet

These are client work still to do: virtualized lists beyond the queue panel's (the other long
lists page in as you scroll), loudness normalization, downloads and playing offline, edits that
survive a closed app (a playlist edit, a play or a queue edit made offline is retried only while
the page is open), recent searches offline, lyric search on the device, and account preferences.
Grid/list view and theme are per-browser for now.

## Catalog worker

The device keeps a copy of every library it can reach, in SQLite on the browser's private file system, so pages, search and queue building work offline ([ADR 0008](../adr/0008-sqlite-device-catalog.md), `design/general.md` §3.1). `src/lib/api/client.ts` stays the one module pages call: it sends catalog reads to `catalog/read.ts`, which asks the worker and gets the JSON the server would send.

- **One owner tab.** The browser's file handle for the database belongs to one dedicated worker and is exclusive, so each tab asks for one Web Lock, and the tab that holds it runs the catalog worker (`client.svelte.ts`). The other tabs send their requests to it over a `BroadcastChannel` and get answers back (`router.ts`), and the queue worker reaches the catalog the same way. A request with no answer when a new owner announces itself is sent again. The owner opens the database only after it holds the lock and releases the pool before it gives the lock up. A new owner opens the files with retries, since the old one may still be closing. When the owner closes, the browser hands the lock to another tab. An owner whose tab is hidden hands over only to a waiting tab that is in view, and only while no snapshot is downloading: tabs tell each other over the channel when an owner is hidden and when a tab in view waits, so on a desktop with several tabs a switch to another site, or to a tab that is not waiting for the catalog, leaves the worker where it is. A waiting tab that goes out of view leaves the queue for the lock and joins it again when shown, so the lock only ever passes to a tab in view; a tab out of view takes it only when no one holds it. This is needed because no browser sends `pagehide` for a tab switch and iOS sends nothing when a tab goes to the background. An owner also lets go when Chromium freezes its page, stopping its worker without waiting for it so the lock is released before the freeze, and takes part again when the page resumes. The worker answers whether a library is on the device, and whether a download is under way, outside its queue of work, so an apply or an index build does not delay them; a tab that still gets no answer takes it as unknown, which offline means asking the device, never saying the library is not downloaded. A tab that was signed in at its last visit joins the election before its first page loads, so a launch at a deep link offline finds the device's copy coming, and, when the device is offline, its reads wait up to five seconds for its own owner. Online, a read goes to the server at once whenever no owner is serving yet, or the owner did not answer, which is not asked again for 30 seconds.
- **One file per library.** Each library is its own SQLite file (`lib-<library>-<generation>.db`) in the `sahpool` VFS, and each file holds its own feed positions and the account's: the library feed's position, its staged pages, and the account feed's position, each with the epoch of the server's run its latest page came in, with only that library's playlists applied. A marker in the file says it is complete and belongs to the signed-in account, and a file of another account is deleted. Replacing a library's file replaces its feed positions and playlists with it. The pool is sized for the libraries plus a temporary file.
- **A first sync is a file.** A library with no copy on the device downloads the server's gzipped snapshot, as it is sent, into a plain OPFS file outside the pool, named for the library and the snapshot's ETag. The worker then inflates the file into the pool in one local loop, since the pool's import holds the whole pool, sized from the database's own header and hashing the gzip as it goes, and deletes the download. The file is published only if the hash is the snapshot's ETag and gzip's own checksum holds; a mismatch, or a gzip that will not inflate, is discarded and downloaded again. A download whose body breaks off, because the network went or the worker closed, is kept: the next try (a `hello`, or the next worker's start) asks for the rest with `Range` and `If-Range`, and appends a `206`; a `200` means the server's file changed, and the download starts over. A start keeps the downloads of libraries not yet on the device and deletes the others, and an account pull deletes those of libraries the account no longer reaches. The snapshot's response carries `X-Jewelcase-Schema` and is never kept in the browser's disk cache, and a tab whose build has another schema stops syncing that library until it reloads. The server builds the snapshot's indexes after copying the rows, which is faster than keeping them up while it fills. The new file is swapped in by generation, so nothing shows until the swap and a library is never partly there. The account feed is pulled from the start into the new file before it is swapped in, and the library feed from the snapshot's position. A download costs the gzip's size of temporary disk (about 118 MB at 500,000 tracks) besides the new copy. Libraries already on the device keep catching up while a new one downloads.
- **Catching up.** The worker pulls the library's feed and the account's on start (when online), on every realtime `hello` and on every `libraryChanged` or `accountChanged` hint. Pages of 1,000 changes are staged and committed as they arrive, so a closed tab resumes where it stopped. Each request sends the epoch the file holds for that feed with its position, and staging a page keeps the page's epoch, so a server restarted mid-catch-up is followed on and a restored one refuses the position (`410`). Once the head is reached a small catch-up is applied in one transaction. A big one is applied a chunk at a time, each its own transaction, so the worker keeps answering; while it runs, pages read from the server and offline reads wait for it to finish (a page says whether it is offline with each read). An apply left part-way, because the tab closed, is finished when the worker starts, and one that fails counts as damage and is rebuilt from a snapshot, except when the device is full: then the copy stays, the error is reported, and the apply is finished on a later pull. One pull runs per feed at a time, and the worker keeps the highest position it has heard of, so a hint that arrives during a pull starts another when it ends. A catch-up that has pulled more than a tenth of a library's entities downloads the snapshot again, and never imports one at or below its own position. When the server's snapshot is not yet newer than the device's, the worker asks for it again a few times before it falls back to the feed. Each account pull re-reads the library list, so a library the account gained is downloaded and one it lost is deleted, and a hint for a library not on the list starts an account pull.
- **A copy that cannot be trusted is replaced.** A library the account lost, sign-out, a file of another schema version, a hash that does not match, a corruption error from any query, a failed open, or storage the browser evicted (iOS does) each delete the copy and start again. A `404` from a feed does the same. A `410` downloads the snapshot again, and the old copy serves until the new one is swapped in. A snapshot of another schema version stops that library's sync until the tab reloads, keeping the copy for offline reads. Raising the device schema version also replaces files kept in an older format, such as positions without epochs. The device never runs `quick_check`: it would block the worker for tens of seconds at 500,000 tracks. Sign-out in any tab wipes the catalog for all of them, and logging out wipes the device before leaving the page, also clearing the kept queue lists, and a signed-in visit to `/login` redirects to `/` instead of wiping it. With no private file system (Safari's private windows) the worker answers nothing and pages read from the server. The page asks the browser to keep its storage and reports a full disk as a quota error.
- **Where a page reads from** (`catalog/read.ts`). A library not fully on the device reads from the server, and offline the page says the library is not on this device yet, never part of it. A library on the device reads from the device alone, at once, even while a catch-up is under way, and the catch-up's changes replace what is shown in place. A query the device fails reads from the server when online, and a page that needs the server says so when offline. A device refuses a playlist read or a recipe whose tracks have not all arrived, instead of showing a shorter list, and the page reads it from the server. Writes read the server: a playlist page's first edit reads the playlist's version from the server before sending, never from the device, because a device that is behind would send a stale one.
- **Live changes.** After applying changes the worker announces that the library changed, to every tab, and the pages that show it reload in place (`live.svelte.ts`): once for each burst, a second after the first announcement. A reload re-reads the loaded rows in pages of 1,000 and swaps in only the rows that changed, so a selection, a drag and the scroll position survive a scan that goes on for minutes. An open playlist follows the account feed alone; the realtime `playlist` message is for third parties. A playlist edit queued or being sent from this device holds a refresh back until the server confirms it or refuses it.
- **Search** goes to the server when it is reachable and to the device when offline, when the server fails, and when the server has not answered in 1.5 seconds. If the device answers first the server's late answer is dropped. The worker builds the core's index from the device database the first time search is used and again after changes, no more often than the server does (a rebuild waits ten times as long as the last one took), so a scan costs the worker a tenth of its time at most. A playlist edit does not stale the index. The device holds the same names the server indexes, so search says nothing about coverage; lyric search is not on the device. At 500,000 tracks the first build took 0.75 to 0.8 s unthrottled and 4 s throttled, blocks the worker while it runs, and holds about 255 MB of WebAssembly memory afterwards.
- **Queue.** Starting or adding a recipe resolves on the device with the same SQL as the server, hashes with the core and attaches at once ([`design/queue.md`](queue.md#7-where-it-is-built-and-what-is-not-yet)). The server's snapshot names its own list, and the list is downloaded only if the hashes differ. Offline, a start gets the full list rather than the rows the view held, and lists stay in Cache Storage. A search recipe resolves on the device only when the app counts itself offline. A queue row for a track the device does not hold yet asks the server.
- **Measured at 500,000 tracks** (a laptop, with the worker's CPU throttled about 8 times, and 4 times as the target): the worker answers its first query within 200 to 350 ms unthrottled and a second at 4 times, a sorted page or a deep keyset page takes under a millisecond to a few milliseconds, reading every track ID in a recipe's order takes 31 to 49 ms unthrottled (under 200 ms throttled), and starting a 500,000-track recipe takes 145 to 165 ms unthrottled and under a second throttled. The database is about 310 MB, 118 MB gzipped on the wire. The benchmark is in `e2e/catalog-scale.bench.ts` ([`CONTRIBUTING.md`](../CONTRIBUTING.md)).

## The installable app

The app has a web manifest (`static/manifest.webmanifest`) with regular and maskable icons, standalone display, a theme colour and the iOS tags, and a SvelteKit service worker (`src/service-worker.ts`) with no extra library. The icons are a plain placeholder until the project has artwork.

- **The shell is kept per version.** The service worker downloads the build and the app's one page into `shell-<version>` when it installs, so the app opens offline, and answers navigations from it. It takes over the page it was installed from too, so on a first visit the pages not opened yet load offline. A new version installs in the background and takes over as soon as it has installed, with no prompt and no reload of an open page: the page asks the browser to look for one at startup, when it is shown again and when the realtime connection says hello (a server upgrade restarts it), never while working offline, and the next load runs the new build. Open pages keep working, since the worker answers the build's hashed files from the previous version's shell too; it keeps that one older shell and no more, so a page left open across two upgrades must reload to load a new chunk. It is registered in production builds only, since in development it would serve a stale app, and the build writes absolute script paths (`paths.relative: false`) so the one page loads at any deep link.
- **Nothing from the API is kept except images.** What an account may see is the server's to decide on every request.
- **Images** are kept as they are seen, in a Cache Storage cache of at most 2,000 images with the oldest dropped first. The cache outlives app versions, and the account page shows its size. Sign-out empties it. Album and artist art draws its ThumbHash placeholder until the picture loads, and when the picture is not kept and the device is offline. Placeholders decode only near the viewport, so a long list does not decode every row.
- **The last known account and library list** are kept in the browser's local storage, so the app starts with no network, and cleared with the catalog at sign-out. A server that cannot be reached while the network is up, or that does not answer within three seconds (waited once for the whole start, not for each read), falls back on them too. A tab with a last known account joins the catalog election as it starts, so a page opened straight onto a deep link waits briefly (up to a few seconds) for its own owner rather than answering that the library is not on this device.
- Downloads, and choosing between local and stream, are the next part of Stage 10 ([`PLAN.md`](../PLAN.md)).

## Connectivity

`connectivity.svelte.ts` holds where the device stands with the server: online, offline, or working offline. The app counts itself offline when the person chose Work offline, the browser says it has no network, or the realtime connection is lost or has not been made within three seconds of the app starting, as when the server cannot be reached from where the device is.

- **An indicator** shows `Offline` or `Working offline` in the player's status label, and nothing while online. The devices panel says its list may be out of date.
- **Work offline** is a switch on the account page. It is per device: the choice is kept in the browser and shared by its tabs. Turning it on stops every request the app makes: the page's, both workers' and the images' (`gated-fetch.ts`, with art read from the cache through blob URLs), and it closes the realtime connection, pauses playback and drops the audio source. Reads are local, and the catalog worker stops pulling.
- **Surfaces that need the server** say so when offline and are not hidden: the admin and plugin pages and the account page's server-only sections show that they need the server, and an error page says the same for any read only the server can answer.

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
