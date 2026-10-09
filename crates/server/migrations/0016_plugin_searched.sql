-- The searched hook (design/hooks.md, Searched): telling a plugin the searches of users who
-- chose to share them with it, and asking it to forget those they remove
-- (requirements/search.md §6, users.md §7).

-- How many of the user's own tracks, albums, and artists each search matched, counted as it
-- was recorded. A search that found nothing is what a wishlist is made of.
ALTER TABLE recent_searches ADD COLUMN found_tracks INTEGER NOT NULL DEFAULT 0;
ALTER TABLE recent_searches ADD COLUMN found_albums INTEGER NOT NULL DEFAULT 0;
ALTER TABLE recent_searches ADD COLUMN found_artists INTEGER NOT NULL DEFAULT 0;

-- Who shares their searches with which plugin, and since when. Off unless a row is here, and
-- only a connected user can turn it on. Keyed by the plugin's id, so it survives a reinstall
-- as connecting does.
CREATE TABLE plugin_search_sharing (
    plugin_id TEXT    NOT NULL,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    shared_at INTEGER NOT NULL,
    PRIMARY KEY (plugin_id, user_id)
) STRICT, WITHOUT ROWID;

-- What the hook delivers, in order, for users who share with any plugin. It names searches
-- rather than holding them: a search's words live only in `recent_searches`, so one deleted
-- there is never sent. `searched` and `forgotten` go to every plugin the user shares with;
-- `stopped`, written when they turn sharing off, goes to that plugin alone, asking it to
-- forget everything it was sent. Rows age out with recent searches.
CREATE TABLE search_events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL CHECK (kind IN ('searched', 'forgotten', 'stopped')),
    search_id  INTEGER,
    plugin_id  TEXT,
    at         INTEGER NOT NULL,
    CHECK ((kind = 'stopped') = (search_id IS NULL)),
    CHECK ((kind = 'stopped') = (plugin_id IS NOT NULL))
) STRICT;

CREATE INDEX search_events_feed ON search_events (library_id, user_id, seq);
CREATE INDEX search_events_age ON search_events (at);

-- `searchActivity` and `searched` are granted per plugin.
CREATE TABLE plugin_grants_searched (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played', 'playing', 'searchActivity', 'searched')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_grants_searched SELECT plugin_id, permission FROM plugin_grants;
DROP TABLE plugin_grants;
ALTER TABLE plugin_grants_searched RENAME TO plugin_grants;

-- A searched position is a user's own, as in played: the last `search_events.seq` delivered.
CREATE TABLE plugin_cursors_searched (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule', 'played', 'playing', 'searched')),
    user_id    INTEGER NOT NULL DEFAULT 0,
    position   INTEGER NOT NULL DEFAULT 0,
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook, user_id)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_cursors_searched (plugin_id, library_id, hook, user_id, position, failures, retry_at)
SELECT plugin_id, library_id, hook, user_id, position, failures, retry_at FROM plugin_cursors;
DROP TABLE plugin_cursors;
ALTER TABLE plugin_cursors_searched RENAME TO plugin_cursors;
