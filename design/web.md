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
| `pnpm wasm`                   | Build `core-wasm` into `src/lib/core-wasm` (run by the others) |
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
| `src/lib/player.svelte.ts` | Playback: one audio element playing the server-kept queue's current item, moving on by queue edits, and Media Session                                   |
| `src/lib/queue/`           | The queue on this device: a worker holding it through the shared core, and the page's client of it (`design/queue.md`)                                  |
| `src/lib/queue.ts`         | Where playback starts in a view's tracks, and when Previous restarts a track                                                                            |
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
- **The queue** does not exist: the server keeps it (`design/queue.md`), so under `pnpm dev`
  without the server a play starts a queue the mock cannot confirm, and nothing plays.
- Only the read endpoints the client uses are implemented; any other write answers `405`.
- **Accounts** don't exist: the mock reports setup as done, everyone is one user, and logging in
  accepts any password.
- **Plugins** can be installed, granted, and enabled, but never run: **Run now** needs the server,
  which hosts them (`crates/plugins`).

## Not built yet

These are client work still to do: virtualized lists beyond the queue panel's (the other long
lists page in as you scroll), loudness normalization, offline, and account preferences. Grid/list
view and theme are per-browser for now.

## Playing

The player plays the account's queue, which the server keeps and every device holds
(`design/queue.md`). Playing from a view starts a queue, here at once and on the server in the
background: a view that holds its whole list (an album, a playlist) sends its track IDs in the
order shown; a view that pages (songs, an artist's songs, unsorted search results) sends the
query it pages through, and plays the clicked track alone, in a one-track queue under the source's
key, until the server's queue arrives. The player then plays the queue's current item,
looking up its track by ID, and the next item's track ahead of time. The device holding playback follows every change of the current item, whoever
made it; other devices only show it. Next, or a track ending, before the server's queue has
arrived waits for it. Next, Previous and a track ending are `next` and `previous` edits, and the
player plays whatever the queue says is current. A track that cannot play is passed over with
one `jump` to the nearest that can (going back, with `previous` edits to the nearest item in History that
can play; with none, Previous restarts the track and changes nothing), so nothing is reordered or removed
([`requirements/conventions.md` §6](../requirements/conventions.md)). Repeat One replays the
track on this device; Repeat All is the queue's, so a context whose last tracks are missing
starts over, once, before the player gives up. Reports name the queue's item, a play records the
queue's context, and a device that takes playback over plays that item from the queue it already
holds. The state names only the item, so a device that only shows playback looks its track up
through its own copy of the queue. A report waits until the server has the queue it names an
item of, which a start sends in the background, since the server refuses an item it does not
have. A library the account no longer reaches answers its lookups with 404, taken as all its
tracks being absent, so its items are passed over like missing ones.

## The queue panel

Opened from the player bar, over the main area like Lyrics and Devices. It shows and edits the
account's queue, so it works the same whichever device plays: every edit goes through the queue
client, applied here at once and sent on, and the device holding playback follows the queue.

- **Two tabs.** Up Next is the Manual Queue under its own heading (a hint stands in when it is
  empty, so it is plain where Queue Last lands), then the context's items under theirs. History is
  most recent first. Shuffle and repeat (Off, All, One) sit beside the tabs, since the player bar
  has none; shuffle sends a random 32-bit seed.
- **Rows are one fixed height, so the list is virtualized by arithmetic** (`queue/layout.ts`,
  `QueueList.svelte`): the rows to draw come from the scroll position, the worker is asked for
  those items' IDs only, and their tracks come from the batch lookup by item, cached
  (`queue/summaries.ts`). A 500,000-item Up Next draws about twenty rows. The list is drawn at most 10 million pixels tall, since Firefox ignores a taller height, and past that the scroll position is scaled into the whole list. A track out of reach or
  missing shows as "Unavailable", never as an error.
- **Editing.** A grab handle drags one row, or the selected rows together, anywhere in Up Next,
  across the Manual Queue's end too; the handle takes no scroll from a finger, and pressing it
  without moving selects the row. From the keyboard, Space on a title selects, Alt+↑/↓ moves the
  selection (or the row) a place, Delete removes. Alt+↑/↓ gathers a scattered selection into one block (a playlist's list steps each row on its own); presses made before the list has caught up with the last one wait for it, so holding the key moves one place per press. A row's ✕ removes it, or the selection it is in.
  Clicking an Up Next row jumps to it, and everything ahead of it goes to History. In History a
  row plays again (`jump`) or returns to the front of Up Next (`move`).
- **A change that did not stay is explained**: a refused `anchor_gone` or `superseded` shows a
  toast (`requirements/realtime.md` §7), and so does a refused add.

### Adding to the queue

- **Queue Next and Queue Last** are the first two items of the ⋯ menu that opens on every song
  row, on the selection bar, on every album, artist and playlist card, and on their own pages
  (`AddToPlaylistMenu`, so there is one menu, with Add to playlist below). They `add` at
  `manualStart` / `manualEnd` (`queueing.ts`) and say what they did in a toast. With no queue
  there is nothing to add to, and the toast says to play something first.
- **What an add carries.** Songs the device holds go as `tracks` and show at once. An album or
  artist card does not hold its songs, so it goes as a `query` (`{type: tracks, albumId, sort:
  album}`, or the artist's) for the server to read. The queue store cannot apply that locally: it
  keeps it pending without showing it, sends it in turn with every other pending edit (one
  request at a time, in the order made), and shows it when the server's resolved `queueEdit`
  arrives, which retires it. Edits made meanwhile show at once and are replayed on top, so the
  result is the server's order. A playlist is not stored on the server, so it goes as its songs,
  fetched first when only its card is at hand.
- **Drop to insert.** With a mouse, song rows (one, or the selection) and cards drag with the
  browser's own drag and drop (`dragging.svelte.ts`) onto Up Next, which draws the same drop line
  as a move and scrolls near its edges. The gap becomes an anchor with `anchorAt`, so a drop
  above the context's heading is the Manual Queue's end and below it the context's front. Rows
  of a list that reorders by dragging (an unsorted playlist) keep that drag for themselves; touch
  and keyboard use the menu, as a moving finger scrolls.
- **Libraries mix.** The caches of tracks (`queue/summaries.ts`, the player's) are keyed by
  library and track ID, and an item's track is always looked up in its own library.

The e2e fixtures keep a queue per test by running the shared core in Node, and answer the
queue's requests from the page's worker, so they route on the browser context, not the page.
