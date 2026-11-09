# 0008. The device catalog is SQLite, with its SQL shared through `crates/catalog`

- **Date:** 2026-11-06
- **Status:** Proposed
- **Commit:** (when it merges)

## Context
A device keeps the whole catalog so it can browse, search and queue offline, at 500,000 tracks, in a phone browser ([`requirements/offline.md` §1](../requirements/offline.md#1-the-catalog-is-always-there)). The answers must match the server's exactly ([`requirements/general.md` §3.5](../requirements/general.md#35-offline-parity)).

## Decision
- The device holds the catalog in SQLite: `rusqlite` compiled to WebAssembly, stored in the browser's private file system (OPFS) through the `sahpool` VFS, in a dedicated worker. A device copy is a cache of the server and is deleted and downloaded again whenever it cannot be trusted.
- The SQL that reads the catalog (listings, filters, paging, search documents, queue recipes) lives in `crates/catalog` and runs on the server and on the device, over a device schema that is a subset of the server's tables. The core stays pure and small for the page thread.
- A device starts from a gzipped SQLite file the server builds per library, downloaded as it is sent so a cut-short download resumes, and checked by its SHA-256 as it is inflated into the database, then catches up from the library's change feed. A catch-up that would change more than a tenth of a library downloads the file again.

## Alternatives considered
- **Replaying every row as JSON.** At 500,000 tracks it took 240 s on a laptop, against 2 to 9 s to import the file, and froze the worker for seconds at a time.
- **Sync engines (Zero, Replicache, Electric, PowerSync).** They need a Postgres-class source and a second service, or cap the client at tens of thousands of rows.
- **Replicating SQLite files (libSQL, Litestream, session changesets, cr-sqlite).** They cannot leave out rows or columns per user or library, and cr-sqlite looks abandoned.
- **Partial, query-driven sync.** Offline search and sorting would depend on what happened to be loaded.
- **wa-sqlite with several connections.** Its multi-tab VFS has open deadlock reports in Chrome and Safari.
- **A hand-built index in the core.** A second copy of the catalog's rules, with its whole index in memory.

## Consequences
- The web build needs clang to compile SQLite for `wasm32` ([`CONTRIBUTING.md`](../CONTRIBUTING.md)), and the workspace uses `rusqlite` 0.41.
- Only one tab's worker can open the database, so other tabs ask it ([`design/web.md`](../design/web.md#catalog-worker)).
- A column added to the catalog is added to the device schema deliberately, and a test keeps the two in step. Paths and other server-only data never reach a device.
- A 500,000-track library is a file of about 310 MB, 118 MB compressed on the wire. That is about 0.002% of the music it describes, within [`requirements/performance.md` §6](../requirements/performance.md#6-client-footprint); sort indexes are most of it and keep sorts instant.
- The download needs the gzip's size of free disk while it runs, since it is downloaded outside the database and then imported, and published only if its hash matches.
- Offline search still uses the core's in-memory index, built from the device database the first time it is needed. The database holds the catalog on disk, and the index is the one thing that stays in memory (about 255 MB at 500,000 tracks).
