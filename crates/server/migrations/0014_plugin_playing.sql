-- The playing hook: telling the plugins a user connected what they have just started playing
-- (design/hooks.md §1.1).

-- Every play reported before it ended, in the order the server first heard of it. `at` is the
-- server's time of that first report, since a device's own clock, and a device catching up
-- after being offline, could make any play look current. A play first reported already ended
-- was never playing as far as anyone here knew, and is not a start.
CREATE TABLE play_starts (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id    TEXT    NOT NULL REFERENCES plays (id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL,
    library_id INTEGER NOT NULL,
    at         INTEGER NOT NULL
) STRICT;

CREATE INDEX play_starts_feed ON play_starts (library_id, user_id, seq);
CREATE INDEX play_starts_play ON play_starts (play_id);

-- Nothing here is delivered late, so a start an hour old is of no use to anyone: each new one
-- clears those, and the table stays small.
CREATE TRIGGER plays_start AFTER INSERT ON plays WHEN NEW.ended IS NULL
BEGIN
    INSERT INTO play_starts (play_id, user_id, library_id, at)
    VALUES (NEW.id, NEW.user_id, NEW.library_id, NEW.updated_at);
    DELETE FROM play_starts WHERE at < NEW.updated_at - 3600000;
END;

-- `playing` is granted per plugin, like `played`.
CREATE TABLE plugin_grants_playing (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played', 'playing')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_grants_playing SELECT plugin_id, permission FROM plugin_grants;
DROP TABLE plugin_grants;
ALTER TABLE plugin_grants_playing RENAME TO plugin_grants;

-- A playing position is a user's own, as in played: the last `play_starts.seq` delivered, or
-- passed over as stale.
CREATE TABLE plugin_cursors_playing (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule', 'played', 'playing')),
    user_id    INTEGER NOT NULL DEFAULT 0,
    position   INTEGER NOT NULL DEFAULT 0,
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook, user_id)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_cursors_playing (plugin_id, library_id, hook, user_id, position, failures, retry_at)
SELECT plugin_id, library_id, hook, user_id, position, failures, retry_at FROM plugin_cursors;
DROP TABLE plugin_cursors;
ALTER TABLE plugin_cursors_playing RENAME TO plugin_cursors;
