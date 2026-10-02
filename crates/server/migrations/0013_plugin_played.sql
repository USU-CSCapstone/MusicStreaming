-- The played hook: delivering each play, once it ends, to the plugins its user connected
-- (requirements/plugins.md §8).

-- Every play that has ended, in the order it ended. A play is reported many times, and only
-- the first report with an end puts it here, so each is delivered once (analytics.md §6).
CREATE TABLE play_ends (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id    TEXT    NOT NULL REFERENCES plays (id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL,
    library_id INTEGER NOT NULL,
    at         INTEGER NOT NULL
) STRICT;

CREATE INDEX play_ends_feed ON play_ends (library_id, user_id, seq);
CREATE INDEX play_ends_play ON play_ends (play_id);

CREATE TRIGGER plays_end_at_once AFTER INSERT ON plays WHEN NEW.ended IS NOT NULL
BEGIN
    INSERT INTO play_ends (play_id, user_id, library_id, at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
END;

CREATE TRIGGER plays_end AFTER UPDATE OF ended ON plays
WHEN OLD.ended IS NULL AND NEW.ended IS NOT NULL
BEGIN
    INSERT INTO play_ends (play_id, user_id, library_id, at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
END;

-- A played position is a user's own, so one user's failures never hold up another's plays.
-- Every other hook has user 0.
CREATE TABLE plugin_cursors_played (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule', 'played')),
    user_id    INTEGER NOT NULL DEFAULT 0,
    -- tracksChanged: the last `library_changes.seq` delivered. scanFinished: when the last
    -- scan delivered finished. schedule: when it last ran. played: the last `play_ends.seq`
    -- delivered. Zero is before everything.
    position   INTEGER NOT NULL DEFAULT 0,
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook, user_id)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_cursors_played (plugin_id, library_id, hook, position, failures, retry_at)
SELECT plugin_id, library_id, hook, position, failures, retry_at FROM plugin_cursors;
DROP TABLE plugin_cursors;
ALTER TABLE plugin_cursors_played RENAME TO plugin_cursors;
