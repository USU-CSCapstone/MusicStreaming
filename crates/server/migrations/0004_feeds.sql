-- The library change feed (design/general.md §4, design/database.md §5).
--
-- A change row names what changed, never its contents: a client catching up receives each
-- entity's current representation, read at that moment. Rows are written in the same
-- transaction as the change itself, so the feed can never disagree with the data.
--
-- Coalescing happens on write: recording a change first deletes the entity's earlier row, so
-- each entity has at most one — its latest. "Everything after cursor N" is then already the
-- net effect (requirements/realtime.md §6). Deleted entities leave one tombstone row.

CREATE TABLE library_changes (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    entity_type TEXT    NOT NULL CHECK (entity_type IN ('library', 'track', 'album', 'artist', 'tag')),
    entity_id   INTEGER NOT NULL,
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete')),
    changed_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX library_changes_feed ON library_changes (library_id, seq);
CREATE UNIQUE INDEX library_changes_entity ON library_changes (library_id, entity_type, entity_id);
-- The tracksChanged, albumsChanged and artistsChanged hooks each read only their own kind of
-- change, so a hook that has caught up must not step over every newer change of another kind.
CREATE INDEX library_changes_kind_feed ON library_changes (library_id, entity_type, seq);
