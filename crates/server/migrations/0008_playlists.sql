-- Playlists: each user's own, in one library (requirements/playlists.md), and the tracks in them.
-- They are the user's data, not the library's, so a scan or purge writes nothing of them but
-- the artwork, which follows the albums' art (design/database.md §6).

CREATE TABLE playlists (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 'liked' for the user's Liked Songs, whose items are their likes, and 'saved' for the rest
    -- (requirements/playlists.md §11)
    kind         TEXT    NOT NULL DEFAULT 'saved' CHECK (kind IN ('saved', 'liked')),
    title        TEXT    NOT NULL CHECK (title <> ''),
    -- The title as search folds it, which a name filter looks inside, and as the core sorts it
    folded       TEXT    NOT NULL,
    sort_key     BLOB    NOT NULL,
    description  TEXT,
    pinned       INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    -- 1 at creation; every change adds 1, and an edit applies only against the version it was
    -- made on
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL,
    modified_at  INTEGER NOT NULL,
    -- Of every item, missing tracks included, kept in the same write as the items
    track_count  INTEGER NOT NULL DEFAULT 0,
    duration_us  INTEGER NOT NULL DEFAULT 0,
    -- The ID of the playlist's artwork, as the API gives it: the album image of its first track
    -- that has one when it has fewer than four distinct album images, else the mosaic of the
    -- first four ("m" and their IDs). NULL with none. Kept in the same write as the items, and
    -- as a scan or purge changes their albums' art.
    image_key    TEXT,
    -- 1 for a pinned playlist and NULL for the rest, so that every list puts the pinned first in
    -- both orders as the unknown part of a keyset does (design/database.md §4)
    pin_rank     INTEGER GENERATED ALWAYS AS (nullif(pinned, 0)) VIRTUAL,
    UNIQUE (library_id, id)
) STRICT;

CREATE INDEX playlists_by_name ON playlists (user_id, library_id, pin_rank, sort_key, id);
-- A user has one Liked Songs in a library at most
CREATE UNIQUE INDEX playlists_liked ON playlists (user_id, library_id) WHERE kind = 'liked';
CREATE INDEX playlists_by_created ON playlists (user_id, library_id, pin_rank, created_at, id);
CREATE INDEX playlists_by_modified ON playlists (user_id, library_id, pin_rank, modified_at, id);

-- A playlist's tracks in the user's order, a track as often as it was added. An item's `key`
-- orders it among its playlist's; keys start 2^20 apart so an item put between two takes the
-- midpoint, and a playlist whose gap is used up is renumbered, so a move rewrites the rows it
-- moves and not the playlist. The API names an item by its position in this order, not its key.
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL,
    key         INTEGER NOT NULL,
    library_id  INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, key),
    FOREIGN KEY (library_id, playlist_id) REFERENCES playlists (library_id, id) ON DELETE CASCADE,
    -- No cascade: retention keeps any track an item names, so it never dangles
    -- (design/database.md §9)
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id)
) STRICT, WITHOUT ROWID;

CREATE INDEX playlist_items_by_track ON playlist_items (library_id, track_id);

-- Each library a user deleted their Liked Songs in, so it is not made again until they next like
-- a track there (requirements/playlists.md §11)
CREATE TABLE liked_songs_deleted (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, library_id)
) STRICT, WITHOUT ROWID;

-- The answer to a create or an edit that came with an `Idempotency-Key`, kept for a day, so a
-- retry of a request that applied (its answer was lost) gets that answer and not a second
-- apply. Written in the same transaction as the change.
CREATE TABLE idempotency_keys (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    status     INTEGER NOT NULL,
    body       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key)
) STRICT, WITHOUT ROWID;

CREATE INDEX idempotency_keys_by_age ON idempotency_keys (created_at);
