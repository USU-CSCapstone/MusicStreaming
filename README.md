# MusicStreaming

A self-hosted music streaming platform for people who own large, curated
collections but still reach for a commercial service because the experience is
better. It runs as a single server container that scans and indexes a library
of tagged music and streams to web and mobile clients with instant search,
a synced queue, offline playback, and recommendations built from listening history.

The server core is deliberately slim: it serves a well-tagged library and
nothing more. Everything else, from metadata enrichment and scrobbling to
loudness analysis and recommendation strategies, is a plugin built on a stable
extension API so the community can extend both server and clients.

# Development

You need Docker with Compose. Point the server at some music by creating a `.env` file at the
repository root:

```sh
JEWELCASE_MUSIC=/path/to/music
```

Then:

```sh
docker compose up --watch   # start everything, then open http://localhost:5173
docker compose down         # stop it; add --volumes to also delete the database and caches
```

As you save, the web app updates in the browser, and the server rebuilds and restarts. The
first start compiles the server, which takes a minute or so; later starts reuse the build.
Your music is mounted read-only and the containers never write to your checkout. The server's
API is also on http://localhost:8080.

Tests and checks run on your machine as usual: `cargo test` and `cargo clippy` at the root, and
`pnpm test` and `pnpm check` in `web/`. Running `pnpm dev` there on its own serves the web app
against a mock of the API instead.

# Prototype

Build a thin slice of the core server and plugin system to learn where the
complexity lives. Assume a well-tagged library; the database is built from
artist, album, and track tags. No metadata enrichment, mobile, offline, or auth
UI.

## Core server

- **Data model**: libraries, users with library membership, artists, albums,
  tracks, play events, per-user playback state and queue.
- **Track identity**: stable internal id separate from file path. Match files
  on rescan by MusicBrainz ID tag, then path, then a tag hash, so moves and
  retags keep history intact.
- **Scanner**: reads tags, builds the model, incremental rescans. Runs as a job.
- **Job queue**: persistent, resumable, shared by the scanner and plugins.
- **Streaming**: direct play with range requests, on-demand transcode to Opus
  with a disk cache.
- **Search**: over artists, albums, playlists, and tracks with fuzzy matching.
- **HTTP API**: JSON resources for the model above, static token per user,
  every response scoped to the user's libraries.
- **Minimal web client**: browse, search, play, reorderable queue. Enough to
  exercise the API and observe plugins.

## Plugin system

- **Extension points**: event sink, background job, provider, API extension.
- **Execution model**: build the scrobbler in-process and in a sandbox, compare
  deployment, isolation, per-call latency, and permission enforcement, then
  commit to one.
- **Manifest and permissions**: plugins declare API version, shapes, and
  permissions (library read, file read, field-scoped track write, event
  subscriptions, jobs, allowlisted network, routes, KV storage). Enforced at a
  single chokepoint in the host API.
- **Host API**: paged library queries, batch track fetch, file read, permitted
  field writes, event subscribe with cursor, job enqueue, outbound HTTP, KV,
  settings, route registration, logging.
- **Lifecycle**: discovery from a plugins directory, enable/disable at
  runtime, per-call timeouts, crash isolation.
- **Reference plugins**, built only through the public API:
  1. Scrobbler (event sink) — writes completed plays to a log or URL.
  2. Loudness analysis (background job) — computes and writes track gain.
  3. Next-track provider — trivial ranking, the seam for future recommendations.

## Unknowns

- Plugin system design
- Plugin system security controls
- How to derive track identity
- How to handle gapless playback
- How to implement a smart fuzzy search
- How to normalize audio
- What is the split between core server and plugins

## Order

1. Schema, scanner, identity
2. Job queue
3. Streaming, API, web client
4. Execution model spike
5. Plugin host
6. Loudness and next-track plugins
7. Search at scale
