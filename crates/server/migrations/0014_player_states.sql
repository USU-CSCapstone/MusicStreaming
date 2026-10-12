-- Where each account's playback was, so it resumes there after the server restarts
-- (requirements/playback.md §6). The state lives in memory while the server runs, and changed
-- states are written every few seconds, all in one write, and as the server stops. A server
-- that starts again has no device connected to be playing, so every account starts stopped
-- where it was (requirements/realtime.md §6), and nothing more than this is kept.

CREATE TABLE player_states (
    user_id     INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- The device's own name for its place in its queue, until the server keeps the queue.
    item_id     TEXT    NOT NULL,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- No foreign key, as in plays: a track that goes is simply not found.
    track_id    INTEGER NOT NULL,
    position_ms INTEGER NOT NULL CHECK (position_ms >= 0)
) STRICT;
