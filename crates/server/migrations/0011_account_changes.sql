-- The account change feed (design/database.md §5): what changed in one user's own data, read by
-- that user's devices alone. Like `library_changes`, a row names what changed and never its
-- contents, is written in the same transaction as the change, and is coalesced on write, so
-- each thing has at most one row, its latest. A deleted playlist leaves one tombstone and its
-- items' rows go.
--
-- A playlist has one row of kind 'playlist' (`entity_id` the playlist, item_key 0) and one of kind
-- 'playlistItem' for each item, named by its key in `playlist_items`; Liked Songs is a playlist
-- like any other. A hidden track has one of kind 'hiddenTrack' (`entity_id` the track, item_key
-- 0), deleted when it is unhidden. The playlists and the hidden tracks are the user's alone
-- (requirements/playlists.md §1, requirements/conventions.md §8), so only the owner has rows.
-- More kinds join it with plays and queues.

CREATE TABLE account_changes (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('playlist', 'playlistItem', 'hiddenTrack')),
    -- The playlist, or the hidden track. Not a reference: a tombstone outlives what it names
    entity_id   INTEGER NOT NULL,
    item_key    INTEGER NOT NULL DEFAULT 0 CHECK (kind = 'playlistItem' OR item_key = 0),
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete'))
) STRICT;

CREATE INDEX account_changes_feed ON account_changes (user_id, seq);
-- Leads with what changed, so deleting a playlist finds all its rows
CREATE UNIQUE INDEX account_changes_thing
    ON account_changes (user_id, entity_id, kind, item_key);

-- Playlists and hidden tracks from before this migration have no rows yet, so a device would never
-- hear of them. Each playlist gets its upsert, in id order, each item after its playlist, and each
-- hidden track after them, so that a device reading the feed from the start receives all of them.
-- The table is new, so nothing collides.
INSERT INTO account_changes (user_id, library_id, kind, entity_id, item_key, op)
SELECT user_id, library_id, 'playlist', id, 0, 'upsert' FROM playlists ORDER BY id;

INSERT INTO account_changes (user_id, library_id, kind, entity_id, item_key, op)
SELECT p.user_id, i.library_id, 'playlistItem', i.playlist_id, i.key, 'upsert'
FROM playlist_items AS i
JOIN playlists AS p ON p.id = i.playlist_id
ORDER BY i.playlist_id, i.key;

INSERT INTO account_changes (user_id, library_id, kind, entity_id, item_key, op)
SELECT user_id, library_id, 'hiddenTrack', track_id, 0, 'upsert' FROM hidden_tracks
ORDER BY user_id, library_id, hidden_at, track_id;
