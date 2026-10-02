-- Every track that starts is a play, and listen time is how much of it was heard
-- (requirements/analytics.md §1). A device reports a play when it starts and again as it
-- goes, under the ID it chose, so a play is recorded once however often it is reported (§6).

CREATE TABLE plays (
    -- The client's `playId`.
    id             TEXT    PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Kept when the device is forgotten: the play still happened.
    device_id      INTEGER REFERENCES devices (id) ON DELETE SET NULL,
    library_id     INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    track_id       INTEGER NOT NULL,
    started_at     INTEGER NOT NULL,
    listen_time_ms INTEGER NOT NULL CHECK (listen_time_ms >= 0),
    -- NULL while it is still playing.
    ended          TEXT    CHECK (ended IN ('finished', 'skipped', 'stopped')),
    context        TEXT    NOT NULL CHECK (json_valid(context)),
    origin         TEXT    NOT NULL CHECK (origin IN ('manual', 'context', 'automatic')),
    reason         TEXT    CHECK (json_valid(reason)),
    updated_at     INTEGER NOT NULL
) STRICT;

CREATE INDEX plays_user ON plays (user_id, started_at);
CREATE INDEX plays_device ON plays (device_id);
