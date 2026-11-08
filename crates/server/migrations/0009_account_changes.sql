-- The account change feed (design/database.md §5): what changed in one user's own data, read by
-- that user's devices alone. Like `library_changes`, a row names what changed and never its
-- contents, is written in the same transaction as the change, and is coalesced on write, so
-- each thing has at most one row, its latest. A deleted playlist leaves one tombstone and its
-- items' rows go.
--
-- A playlist has one row of kind 'playlist' (item_key 0) and one of kind 'playlistItem' for each
-- item, named by its key in `playlist_items`. The playlist and its items are the user's alone
-- (requirements/playlists.md §1), so only the owner has rows. More kinds join it with plays and
-- queues.

CREATE TABLE account_changes (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('playlist', 'playlistItem')),
    -- Not a reference: a tombstone outlives its playlist
    playlist_id INTEGER NOT NULL,
    item_key    INTEGER NOT NULL DEFAULT 0 CHECK (kind = 'playlistItem' OR item_key = 0),
    op          TEXT    NOT NULL CHECK (op IN ('upsert', 'delete'))
) STRICT;

CREATE INDEX account_changes_feed ON account_changes (user_id, seq);
-- Leads with the playlist, so deleting one finds all its rows
CREATE UNIQUE INDEX account_changes_thing
    ON account_changes (user_id, playlist_id, kind, item_key);
