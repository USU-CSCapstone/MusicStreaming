-- Change feeds (design/general.md §4) and per-user listening aggregates.
--
-- A change row names what changed, never its contents: a client catching up receives each
-- entity's current representation, read at that moment. Rows are written in the same
-- transaction as the change itself, so the feed can never disagree with the data.
--
-- Coalescing happens on write: recording a change first deletes the entity's earlier row, so
-- each entity has at most one — its latest. "Everything after cursor N" is then already the
-- net effect (requirements/realtime.md §6). Deleted entities leave one tombstone row, dropped
-- once older than the retention horizon; the feed's horizon then moves past it and older
-- cursors answer 410.

CREATE TABLE library_changes (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    entity_type TEXT    NOT NULL CHECK (entity_type IN ('library', 'track', 'album', 'artist', 'tag')),
    entity_id   INTEGER NOT NULL,
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete')),
    at          INTEGER NOT NULL
) STRICT;

CREATE INDEX library_changes_feed ON library_changes (library_id, seq);
CREATE UNIQUE INDEX library_changes_entity ON library_changes (library_id, entity_type, entity_id);

-- The account feed. Entity types grow as the personal sections of the API are built.
CREATE TABLE account_changes (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    entity_type TEXT    NOT NULL CHECK (entity_type IN ('library', 'user', 'accountSettings', 'device', 'personal')),
    entity_id   INTEGER NOT NULL,
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete')),
    at          INTEGER NOT NULL
) STRICT;

CREATE INDEX account_changes_feed ON account_changes (user_id, seq);
CREATE UNIQUE INDEX account_changes_entity ON account_changes (user_id, entity_type, entity_id);

-- The `personal` object on content (play count, last played).
CREATE TABLE personal_stats (
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id     INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    entity_type    TEXT    NOT NULL CHECK (entity_type IN ('track', 'album', 'artist', 'tag', 'playlist')),
    entity_id      INTEGER NOT NULL,
    play_count     INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER,
    PRIMARY KEY (user_id, entity_type, entity_id)
) STRICT, WITHOUT ROWID;

-- Sorting by play count: played items page from here, then unplayed ones by name.
CREATE INDEX personal_stats_by_plays ON personal_stats (user_id, library_id, entity_type, play_count, entity_id);
CREATE INDEX personal_stats_by_last_played ON personal_stats (user_id, library_id, entity_type, last_played_at, entity_id);
