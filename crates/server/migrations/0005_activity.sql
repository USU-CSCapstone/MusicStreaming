-- Listening and searching: plays and their outboxes for the played and playing hooks, where
-- each account's playback was, recent searches, and the searched hook's outbox.

-- ───────────────────────────── Plays ─────────────────────────────

-- Every track that starts is a play, and listen time is how much of it was heard
-- (requirements/analytics.md §1). A device reports a play when it starts and again as it
-- goes, under the ID it chose, so a play is recorded once however often it is reported (§6).
CREATE TABLE plays (
    -- The client's `playId`. Any device can choose any ID, so it names a play only among its
    -- user's own, and one user's report can never reach another's play.
    id             TEXT    NOT NULL,
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
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (user_id, id)
) STRICT;

CREATE INDEX plays_by_user ON plays (user_id, started_at);
CREATE INDEX plays_by_device ON plays (device_id);

-- Every play that has ended, in the order it ended, for the played hook (requirements/plugins.md
-- §8). A play is reported many times, and only the first report with an end puts it here, so
-- each is delivered once (analytics.md §6).
CREATE TABLE play_ends (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id    TEXT    NOT NULL,
    user_id    INTEGER NOT NULL,
    library_id INTEGER NOT NULL,
    ended_at   INTEGER NOT NULL,
    FOREIGN KEY (user_id, play_id) REFERENCES plays (user_id, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX play_ends_feed ON play_ends (library_id, user_id, seq);
CREATE INDEX play_ends_by_play ON play_ends (user_id, play_id);

CREATE TRIGGER plays_end_at_once AFTER INSERT ON plays WHEN NEW.ended IS NOT NULL
BEGIN
    INSERT INTO play_ends (play_id, user_id, library_id, ended_at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
END;

CREATE TRIGGER plays_end AFTER UPDATE OF ended ON plays
WHEN OLD.ended IS NULL AND NEW.ended IS NOT NULL
BEGIN
    INSERT INTO play_ends (play_id, user_id, library_id, ended_at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
END;

-- Every play reported before it ended, in the order the server first heard of it, for the
-- playing hook (design/hooks.md §1.1). `reported_at` is the server's time of that first report,
-- since a device's own clock, and a device catching up after being offline, could make any play
-- look current. A play first reported already ended was never playing as far as anyone here
-- knew, and is not a start.
CREATE TABLE play_starts (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id     TEXT    NOT NULL,
    user_id     INTEGER NOT NULL,
    library_id  INTEGER NOT NULL,
    reported_at INTEGER NOT NULL,
    FOREIGN KEY (user_id, play_id) REFERENCES plays (user_id, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX play_starts_feed ON play_starts (library_id, user_id, seq);
CREATE INDEX play_starts_by_play ON play_starts (user_id, play_id);
CREATE INDEX play_starts_age ON play_starts (reported_at);

-- Nothing here is delivered late, so a start an hour old is of no use to anyone: each new one
-- clears those, and the table stays small.
CREATE TRIGGER plays_start AFTER INSERT ON plays WHEN NEW.ended IS NULL
BEGIN
    INSERT INTO play_starts (play_id, user_id, library_id, reported_at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
    DELETE FROM play_starts WHERE reported_at < NEW.updated_at - 3600000;
END;

-- ───────────────────────────── Player ─────────────────────────────

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

-- ───────────────────────────── Searches ─────────────────────────────

-- Recent searches (requirements/search.md §6): what the search box offers when it is empty.
-- Each user's own, per library, and kept only for a recent window (`api/recent_searches.rs`),
-- so there is no permanent search history.
CREATE TABLE recent_searches (
    -- Never reused, so an ID names one search even after the same words are searched again.
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    query         TEXT    NOT NULL,
    -- The query's words as search compares them, ignoring case and accents. Searching the same
    -- words again replaces the entry.
    folded        TEXT    NOT NULL,
    -- The result the user acted on, if any.
    selected_type TEXT    CHECK (selected_type IN ('track', 'album', 'artist')),
    selected_id   INTEGER,
    searched_at   INTEGER NOT NULL,
    -- How many of the user's own tracks, albums, and artists the search matched, counted as it
    -- was recorded. A search that found nothing is what a wishlist is made of.
    found_tracks  INTEGER NOT NULL DEFAULT 0,
    found_albums  INTEGER NOT NULL DEFAULT 0,
    found_artists INTEGER NOT NULL DEFAULT 0,
    CHECK ((selected_type IS NULL) = (selected_id IS NULL))
) STRICT;

CREATE UNIQUE INDEX recent_searches_query ON recent_searches (user_id, library_id, folded);
CREATE INDEX recent_searches_by_user ON recent_searches (user_id, library_id, searched_at);
-- For sweeping away everyone's searches once they age out of the window.
CREATE INDEX recent_searches_age ON recent_searches (searched_at);

-- What the searched hook delivers, in order, for users who share their searches with any plugin
-- (design/hooks.md §1.2; requirements/search.md §6, users.md §7). It names searches rather
-- than holding them: a search's words live only in `recent_searches`, so one deleted there is
-- never sent. `searched` and `forgotten` go to every plugin the user shares with; `stopped`,
-- written when they turn sharing off, goes to that plugin alone, asking it to forget everything
-- it was sent. Rows age out with recent searches.
CREATE TABLE search_events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL CHECK (kind IN ('searched', 'forgotten', 'stopped')),
    search_id  INTEGER,
    plugin_id  TEXT,
    created_at INTEGER NOT NULL,
    CHECK ((kind = 'stopped') = (search_id IS NULL)),
    CHECK ((kind = 'stopped') = (plugin_id IS NOT NULL))
) STRICT;

CREATE INDEX search_events_feed ON search_events (library_id, user_id, seq);
CREATE INDEX search_events_age ON search_events (created_at);
-- Who has stopped sharing with which plugin, which the searched hook must still tell.
CREATE INDEX search_events_stopped ON search_events (plugin_id, user_id) WHERE kind = 'stopped';
