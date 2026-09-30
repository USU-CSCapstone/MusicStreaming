-- What a plugin keeps between runs, per library (requirements/plugins.md §2.1): its own keys
-- and values, which only it reads. It goes with the plugin when it is uninstalled.

CREATE TABLE plugin_state (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      BLOB    NOT NULL,
    PRIMARY KEY (plugin_id, library_id, key)
) STRICT, WITHOUT ROWID;
