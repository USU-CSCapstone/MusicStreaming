-- Libraries and their root folders (requirements/libraries.md §1–§3).

CREATE TABLE libraries (
    id                    INTEGER PRIMARY KEY,
    name                  TEXT    NOT NULL,
    -- Patterns to exclude from scanning, relative to the roots
    excludes              TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(excludes) AND json_type(excludes) = 'array'),
    -- Whether to watch the library for changes
    watch                 INTEGER NOT NULL DEFAULT 1 CHECK (watch IN (0, 1)),
    -- The time between scheduled scans, in minutes; NULL disables them
    scan_interval_minutes INTEGER DEFAULT 1440 CHECK (scan_interval_minutes > 0),
    -- Counts and durations maintained by the scanner in the same transaction as the content it changes
    track_count           INTEGER NOT NULL DEFAULT 0,
    album_count           INTEGER NOT NULL DEFAULT 0,
    artist_count          INTEGER NOT NULL DEFAULT 0,
    duration_us           INTEGER NOT NULL DEFAULT 0,
    -- Oldest library-feed position still answerable; older cursors resync (see 0004).
    feed_horizon          INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
) STRICT;

CREATE TABLE library_roots (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    path       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    removed_at INTEGER,
    -- Target for tracks' and images' foreign keys.
    UNIQUE (library_id, id)
) STRICT;

-- One active root per path, across all libraries.
CREATE UNIQUE INDEX library_roots_path ON library_roots (path) WHERE removed_at IS NULL;

-- ───────────────────────────── Scanning ─────────────────────────────

-- One row per scan, mirroring the API's Scan object (design/scanning.md §9, §13).
CREATE TABLE scans (
    id              INTEGER PRIMARY KEY,
    library_id      INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    trigger         TEXT    NOT NULL CHECK (trigger IN ('initial', 'watch', 'scheduled', 'manual', 'reconfigure', 'restore')),
    state           TEXT    NOT NULL CHECK (state IN ('queued', 'running', 'completed', 'cancelled', 'suspended')),
    -- The scopes as a JSON array of {root, path, depth}; never queried by field
    scopes          TEXT    NOT NULL CHECK (json_valid(scopes) AND json_type(scopes) = 'array'),
    -- Resume cursor: the scope being worked and the last directory fully persisted in it
    cursor_scope    INTEGER,
    cursor_dir      TEXT,
    files_seen      INTEGER NOT NULL DEFAULT 0,
    files_processed INTEGER NOT NULL DEFAULT 0,
    added           INTEGER NOT NULL DEFAULT 0,
    updated         INTEGER NOT NULL DEFAULT 0,
    moved           INTEGER NOT NULL DEFAULT 0,
    missing         INTEGER NOT NULL DEFAULT 0,
    problems        INTEGER NOT NULL DEFAULT 0,
    current_path    TEXT,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    finished_at     INTEGER
) STRICT;

CREATE INDEX scans_by_library ON scans (library_id, created_at, id);
CREATE INDEX scans_unfinished ON scans (library_id, id) WHERE state IN ('queued', 'running');

-- Problems grouped so a systemic issue reads as one row (requirements/scanning.md §10).
CREATE TABLE scan_problem_groups (
    id            INTEGER PRIMARY KEY,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('unreadable', 'permissionDenied', 'malformedTags', 'unsupportedEncoding', 'corruptAudio', 'stalled')),
    -- Kind, root, and the detail with paths and numbers stripped (design/scanning.md §13)
    group_key     TEXT    NOT NULL,
    summary       TEXT    NOT NULL,
    count         INTEGER NOT NULL DEFAULT 0,
    first_seen_at INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    UNIQUE (library_id, group_key),
    UNIQUE (library_id, id)
) STRICT;

-- One row per path with a problem; cleared when the path scans cleanly.
CREATE TABLE scan_problems (
    library_id INTEGER NOT NULL,
    group_id   INTEGER NOT NULL,
    root_id    INTEGER NOT NULL,
    -- Relative to the root, like tracks.path
    path       TEXT    NOT NULL,
    detail     TEXT    NOT NULL,
    seen_at    INTEGER NOT NULL,
    PRIMARY KEY (root_id, path),
    FOREIGN KEY (library_id, group_id) REFERENCES scan_problem_groups (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, root_id) REFERENCES library_roots (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX scan_problems_by_group ON scan_problems (group_id, seen_at);
