-- The albumsChanged and artistsChanged hooks (design/hooks.md): running a plugin as albums or
-- artists change, read from the library change feed (`0004_feeds.sql`) as tracksChanged reads
-- tracks. Both are granted per library.

CREATE TABLE plugin_library_grants_albums (
    plugin_id  TEXT    NOT NULL,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    permission TEXT    NOT NULL CHECK (permission IN ('libraryRead', 'libraryAdd', 'libraryChange', 'tracksChanged', 'scanFinished', 'albumsChanged', 'artistsChanged')),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_library_grants_albums SELECT plugin_id, library_id, permission FROM plugin_library_grants;
DROP TABLE plugin_library_grants;
ALTER TABLE plugin_library_grants_albums RENAME TO plugin_library_grants;

-- albumsChanged and artistsChanged: the last `library_changes.seq` of their kind delivered.
CREATE TABLE plugin_cursors_albums (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule', 'played', 'playing', 'searched', 'albumsChanged', 'artistsChanged')),
    user_id    INTEGER NOT NULL DEFAULT 0,
    position   INTEGER NOT NULL DEFAULT 0,
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook, user_id)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_cursors_albums (plugin_id, library_id, hook, user_id, position, failures, retry_at)
SELECT plugin_id, library_id, hook, user_id, position, failures, retry_at FROM plugin_cursors;
DROP TABLE plugin_cursors;
ALTER TABLE plugin_cursors_albums RENAME TO plugin_cursors;
