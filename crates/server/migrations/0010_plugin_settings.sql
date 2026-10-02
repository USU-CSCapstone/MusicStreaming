-- What admins set for each plugin, as its manifest declares (requirements/plugins.md §6):
-- server-wide, and per library where a library's differ. Configuration survives a reinstall
-- (§5), so it is keyed by the plugin's id, not its row. Secrets are kept here to be used,
-- and the API never returns them.

CREATE TABLE plugin_settings (
    plugin_id  TEXT    NOT NULL,
    -- Zero for the settings of every library; a library's own override them.
    library_id INTEGER NOT NULL DEFAULT 0,
    settings   TEXT    NOT NULL CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    PRIMARY KEY (plugin_id, library_id)
) STRICT, WITHOUT ROWID;
