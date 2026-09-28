# 0004. SQLite as the only database

- **Date:** 2026-09-25 11:17 -0600
- **Status:** Accepted
- **Commit:** 8124ba6 (schema), eed6e3d (rusqlite)

## Context
Backups must be a single directory and upgrades a tag change, with no separate database server to run. The workload is read-heavy.

## Decision
- SQLite through `rusqlite` with the `bundled` feature, in WAL mode, with foreign keys on for every connection.
- Migrations are plain SQL files in `crates/server/migrations/`, embedded in the binary and applied at startup, tracked by `PRAGMA user_version`.
- Tables are `STRICT`.

## Alternatives considered
- **PostgreSQL.** It would add a second service, turn backup into `pg_dump` instead of copying `state/`, and make major-version upgrades a dump-and-restore instead of an image-tag change. For a self-hosted application, SQLite simplifies the setup.

## Consequences
- There is a single writer, so write batches must stay short.
- Correctness leans on foreign keys, so every connection must enable them.
