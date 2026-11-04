-- The tracks each user hid, per library: gone from their library, shown only in Hidden Songs
-- (requirements/conventions.md §8). The user's data, not the library's, like their playlists.

CREATE TABLE hidden_tracks (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL,
    hidden_at  INTEGER NOT NULL,
    PRIMARY KEY (user_id, library_id, track_id),
    -- A purged track takes its hide with it: unlike a playlist, a hide keeps no missing track
    -- (requirements/scanning.md §8).
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- Hidden Songs, newest first
CREATE INDEX hidden_tracks_by_time ON hidden_tracks (user_id, library_id, hidden_at, track_id);
-- The cascade from a purged track
CREATE INDEX hidden_tracks_by_track ON hidden_tracks (library_id, track_id);
