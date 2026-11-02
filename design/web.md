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
| `src/lib/player.svelte.ts` | Playback: one audio element, the play context, and Media Session                                                                                        |
| `src/lib/queue/`           | The queue on this device: a worker holding it through the shared core, and the page's client of it (`design/queue.md`)                                  |
| `src/lib/queue.ts`         | Moving through a context: next/previous and skipping missing tracks                                                                                     |
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

These are client work still to do: virtualized lists (long lists currently page in as you scroll),
the queue panel, loudness normalization, offline, and account preferences. Grid/list view and
theme are per-browser for now.

## Queue worker

`src/lib/queue/` holds the account's queue in a web worker, so loading and editing a queue of any
size stays off the page's thread (`design/general.md` §7.1). `worker.ts` is a shell around
`QueueStore` (`store.ts`, plain TypeScript, so it also runs in tests); `client.svelte.ts` is the
page's side, and `protocol.ts` is what they say. The store runs the shared core, built to
WebAssembly by `pnpm wasm`, and holds no rule about the queue's order. Its state holds no tracks:
the core knows each source's length and, once attached, its list, which later steps fill in
(`design/queue.md` §3).

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
  edit, to the page, which shows nothing yet (`client.refusal`): the core refused it here
  (`anchor_gone`, `invalid`); the server refused it (`anchor_gone`, `play_gone`, 422 and the
  like), and the view is rebuilt without it; a `queueEdit` heard first took what a pending edit
  needed (the core's code on the replay); a new queue replaced it (`superseded`); or the server
  answered it but, after a restart, no longer has it (`lost`), which is told by its version
  being past the snapshot's. A lost edit is not sent again, since that could apply it twice.
- **An `add` of a `recipe`,** such as an album, has no length here, so it is only sent. It shows
  when its `queueEdit`, carrying the source as the server registered it, arrives, and edits made
  after it replay on top.
- **`queueReplaced`** for a queue other than the one held fetches the new queue; pending edits
  for the old one are dropped as `superseded`. `queuesChanged` is not the worker's.
- **Connection.** `connect()` spawns the worker and passes it `hello`, `queueEdit` and
  `queueReplaced`. A request that fails, or answers 429 or 5xx, is tried again after a wait.
