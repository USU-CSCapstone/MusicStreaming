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
    id                TEXT    NOT NULL,
    user_id           INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Kept when the device is forgotten: the play still happened.
    device_id         INTEGER REFERENCES devices (id) ON DELETE SET NULL,
    -- Cleared when the library is deleted: the play is still its user's (analytics.md §8).
    library_id        INTEGER REFERENCES libraries (id) ON DELETE SET NULL,
    -- No foreign key: a purged track leaves its plays, which keep the snapshot below.
    track_id          INTEGER NOT NULL,
    -- The track as the play's latest report found it, which history shows and statistics
    -- count once the track or its library is gone (requirements/analytics.md §8). Its
    -- artists, album and genres are kept as the catalog names and matches them
    -- (design/database.md §3, §6): the artists' names in credit order as a JSON array, null
    -- for the unknown artist, and their name keys in the same order; the album's title, NULL
    -- for the unknown album, and its title and artists keys; the genres' keys in key order.
    title             TEXT    NOT NULL,
    artists           TEXT    NOT NULL CHECK (json_valid(artists) AND json_type(artists) = 'array'),
    artist_keys       TEXT    NOT NULL CHECK (json_valid(artist_keys) AND json_type(artist_keys) = 'array'
                                          AND json_array_length(artist_keys) = json_array_length(artists)),
    album             TEXT,
    album_title_key   TEXT    NOT NULL,
    album_artists_key TEXT    NOT NULL,
    genre_keys        TEXT    NOT NULL CHECK (json_valid(genre_keys) AND json_type(genre_keys) = 'array'),
    release_date      TEXT,
    duration_us       INTEGER NOT NULL,
    started_at        INTEGER NOT NULL,
    listen_time_ms    INTEGER NOT NULL CHECK (listen_time_ms >= 0),
    -- NULL while it is still playing.
    ended             TEXT    CHECK (ended IN ('finished', 'skipped', 'stopped')),
    context           TEXT    NOT NULL CHECK (json_valid(context)),
    origin            TEXT    NOT NULL CHECK (origin IN ('manual', 'context', 'automatic')),
    reason            TEXT    CHECK (json_valid(reason)),
    updated_at        INTEGER NOT NULL,
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
    -- A deleted library's events go with it, though its plays stay.
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('started', 'ended')),
    reported_at INTEGER NOT NULL,
    FOREIGN KEY (user_id, play_id) REFERENCES plays (user_id, id) ON DELETE CASCADE
) STRICT;

-- Each hook reads only its own kind, so one that has caught up never steps over the other's:
-- ends pile up as a device that was offline catches up, and starts as someone listens.
CREATE INDEX play_events_feed ON play_events (library_id, user_id, kind, seq);
CREATE INDEX play_events_by_play ON play_events (user_id, play_id);
CREATE INDEX play_events_started_age ON play_events (reported_at) WHERE kind = 'started';
-- For the hourly sweep, which deletes ends once they are 30 days old (design/database.md §9).
CREATE INDEX play_events_ended_age ON play_events (reported_at) WHERE kind = 'ended';

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
-- (requirements/playback.md §6). The state lives in memory while the server runs. Changed
-- states are written together every few seconds, an account playing on through the same track
-- only every minute, and once more as the server stops (`api/player/saved.rs`). A server that
-- starts again has no device connected to be playing, so every account starts stopped where it
-- was (requirements/realtime.md §6), and nothing more than this is kept.
CREATE TABLE player_states (
    user_id     INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- The device's own name for its place in its queue, until the server keeps the queue.
    item_id     TEXT    NOT NULL,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- No foreign key, as in plays: a track that goes is simply not found.
    track_id    INTEGER NOT NULL,
    position_ms INTEGER NOT NULL CHECK (position_ms >= 0)
) STRICT;

-- ───────────────────────────── Queues ─────────────────────────────

-- Each account's queue (requirements/queue.md §6–§8, design/queue.md): one per account, since a
-- new context replaces it. The server keeps it in memory and saves it in the background within
-- about a second of a change, and once more as the server stops (`api/queue/saved.rs`).
CREATE TABLE queues (
    user_id  INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- The `queueId` the device that started the queue chose. Any device can choose any ID, so
    -- it names a queue only among its user's own (design/database.md §1).
    queue_id TEXT    NOT NULL,
    -- Zero when the queue starts; each edit adds one.
    version  INTEGER NOT NULL CHECK (version >= 0),
    -- What the queue started from, as the device sent it, recorded against plays.
    context  TEXT    NOT NULL CHECK (json_valid(context) AND json_type(context) = 'object'),
    -- The core's queue without its sources: the order, the cursor, History, the shuffle seed.
    state    TEXT    NOT NULL CHECK (json_valid(state) AND json_type(state) = 'object')
) STRICT;

-- A queue's sources, each written once as it joins the queue, so a shuffle or a move saves no
-- more than `queues.state`. They are numbered in the order they joined, which numbers the
-- items. No foreign key to the library: once it is deleted its tracks are simply not found,
-- as in `player_states`, and random IDs keep a later library from inheriting them.
CREATE TABLE queue_sources (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL CHECK (position >= 0),
    -- The source key the device that added it chose, which names its items.
    key        TEXT    NOT NULL,
    -- As the API shows it, and never joined: it is read back only to be handed out again.
    library_id TEXT    NOT NULL,
    -- Track IDs as strings, as the API shows them, in the order they were resolved.
    tracks     TEXT    NOT NULL CHECK (json_valid(tracks) AND json_type(tracks) = 'array'),
    PRIMARY KEY (user_id, position)
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
-- it was sent, and is kept until it is delivered or that plugin is uninstalled. The other rows
-- age out with recent searches (design/database.md §9).
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
