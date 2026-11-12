-- Recent searches join the account change feed (design/database.md §5), so a device keeps its
-- user's and records, removes and clears them offline as online (requirements/offline.md §6). A
-- recent search has one row of kind 'recentSearch' (`entity_id` the search, item_key 0), which
-- becomes a tombstone once the search is deleted, however it goes.
--
-- SQLite cannot change a CHECK, so the table is made again with the new kind, keeping every row
-- and its `seq`.

CREATE TABLE account_changes_new (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL
        CHECK (kind IN ('playlist', 'playlistItem', 'hiddenTrack', 'recentSearch')),
    -- The playlist, the hidden track, or the search. Not a reference: a tombstone outlives what
    -- it names
    entity_id   INTEGER NOT NULL,
    item_key    INTEGER NOT NULL DEFAULT 0 CHECK (kind = 'playlistItem' OR item_key = 0),
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete')),
    -- The row's place in its user's feed, which clients see (0013_feed_positions.sql)
    position    INTEGER NOT NULL DEFAULT 0
) STRICT;

INSERT INTO account_changes_new (seq, user_id, library_id, kind, entity_id, item_key, op, position)
SELECT seq, user_id, library_id, kind, entity_id, item_key, op, position FROM account_changes;

DROP TABLE account_changes;
ALTER TABLE account_changes_new RENAME TO account_changes;

CREATE INDEX account_changes_feed ON account_changes (user_id, position);
-- Leads with what changed, so deleting a playlist finds all its rows
CREATE UNIQUE INDEX account_changes_thing
    ON account_changes (user_id, entity_id, kind, item_key);

-- The searches kept before this migration get their upserts after each user's head, oldest
-- first, so a device catching up from where it is hears of them.
INSERT INTO account_changes (user_id, library_id, kind, entity_id, item_key, op, position)
SELECT s.user_id, s.library_id, 'recentSearch', s.id, 0, 'upsert',
       u.feed_head + row_number() OVER (PARTITION BY s.user_id ORDER BY s.searched_at, s.id)
FROM recent_searches AS s
JOIN users AS u ON u.id = s.user_id
ORDER BY s.user_id, s.searched_at, s.id;

UPDATE users SET feed_head = feed_head +
    (SELECT count(*) FROM recent_searches WHERE user_id = users.id);
