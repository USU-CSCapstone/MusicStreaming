-- Installed plugins, what the admin granted them, and where they are enabled
-- (requirements/plugins.md §4–5). Each plugin's code is `state/plugins/<id>.wasm`.

CREATE TABLE plugins (
    id           TEXT    PRIMARY KEY,
    -- As the plugin file carries it (design/plugins.md §6), validated at install.
    manifest     TEXT    NOT NULL CHECK (json_valid(manifest)),
    -- Where it was installed from; null for an uploaded file.
    source_url   TEXT,
    installed_at INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

-- Grants outlive an uninstall, so a reinstall needs no re-approval (§5): they are keyed by
-- the plugin's id, not its row.
CREATE TABLE plugin_grants (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;

-- The permissions granted per library (§4.1).
CREATE TABLE plugin_library_grants (
    plugin_id  TEXT    NOT NULL,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    permission TEXT    NOT NULL CHECK (permission IN ('libraryRead', 'libraryWrite')),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;

-- Enabled per library, independently (§5). No row is disabled.
CREATE TABLE plugin_libraries (
    plugin_id       TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id      INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    enabled         INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    -- Why the server disabled it, such as a required permission revoked; null if the admin did.
    disabled_reason TEXT,
    PRIMARY KEY (plugin_id, library_id)
) STRICT, WITHOUT ROWID;
