-- Listening and searching: plays and the played and playing hooks' outbox, where each account's
-- playback was, recent searches, and the searched hook's outbox. Each hook reads its own
-- domain's outbox (design/general.md §4).

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
    -- No foreign key: a purged track leaves its plays, which keep the snapshot below.
    track_id       INTEGER NOT NULL,
    -- The track as the play's latest report found it, which history shows in place of the
    -- live track once the track is purged (requirements/analytics.md §8): its title, its
    -- artists' names in credit order as a JSON array without the unknown artist, and its
    -- album's title, NULL for the unknown album.
    title          TEXT    NOT NULL,
    artists        TEXT    NOT NULL CHECK (json_valid(artists) AND json_type(artists) = 'array'),
    album          TEXT,
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

-- What the played and playing hooks deliver, in the order the server heard it: each play that
-- ended, for played (requirements/plugins.md §8), and each play reported before it ended, for
-- playing (design/hooks.md §1.1). A play is reported many times, and only the first report with
-- an end adds its end, so each is delivered once (analytics.md §6). A play first reported
-- already ended was never playing as far as anyone here knew, and has no start. `reported_at`
-- is when the server heard the start or the end, since a device's own clock, and a device
-- catching up after being offline, could make any play look current.
CREATE TABLE play_events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id     TEXT    NOT NULL,
    user_id     INTEGER NOT NULL,
    library_id  INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('started', 'ended')),
    reported_at INTEGER NOT NULL,
    FOREIGN KEY (user_id, play_id) REFERENCES plays (user_id, id) ON DELETE CASCADE
) STRICT;

-- Each hook reads only its own kind, so one that has caught up never steps over the other's:
-- ends pile up as a device that was offline catches up, and starts as someone listens.
CREATE INDEX play_events_feed ON play_events (library_id, user_id, kind, seq);
CREATE INDEX play_events_by_play ON play_events (user_id, play_id);
CREATE INDEX play_events_started_age ON play_events (reported_at) WHERE kind = 'started';

-- Nothing is announced as playing late, so a start an hour old is of no use to anyone: each new
-- play clears those, and the starts stay few.
CREATE TRIGGER plays_first_report AFTER INSERT ON plays
BEGIN
    INSERT INTO play_events (play_id, user_id, library_id, kind, reported_at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, iif(NEW.ended IS NULL, 'started', 'ended'),
            NEW.updated_at);
    DELETE FROM play_events WHERE kind = 'started' AND reported_at < NEW.updated_at - 3600000;
END;

CREATE TRIGGER plays_end AFTER UPDATE OF ended ON plays
WHEN OLD.ended IS NULL AND NEW.ended IS NOT NULL
BEGIN
    INSERT INTO play_events (play_id, user_id, library_id, kind, reported_at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, 'ended', NEW.updated_at);
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
    seq           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('searched', 'forgotten', 'stopped')),
    search_id     INTEGER,
    plugin_id     TEXT,
    -- For a search, how many of the user's own tracks, albums, and artists it matched, counted
    -- as it was recorded. A search that found nothing is what a wishlist is made of.
    found_tracks  INTEGER,
    found_albums  INTEGER,
    found_artists INTEGER,
    created_at    INTEGER NOT NULL,
    CHECK ((kind = 'stopped') = (search_id IS NULL)),
    CHECK ((kind = 'stopped') = (plugin_id IS NOT NULL)),
    CHECK (kind <> 'searched'
           OR (found_tracks IS NOT NULL AND found_albums IS NOT NULL AND found_artists IS NOT NULL)),
    CHECK (kind = 'searched' OR coalesce(found_tracks, found_albums, found_artists) IS NULL)
) STRICT;

CREATE INDEX search_events_feed ON search_events (library_id, user_id, seq);
CREATE INDEX search_events_age ON search_events (created_at);
-- Who has stopped sharing with which plugin, which the searched hook must still tell.
CREATE INDEX search_events_stopped ON search_events (plugin_id, user_id) WHERE kind = 'stopped';
