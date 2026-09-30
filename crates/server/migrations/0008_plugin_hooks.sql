-- Hooks: running a plugin when something happens (requirements/plugins.md §8). A hook is
-- requested and approved exactly as a permission is, so it is kept with the grants. The
-- CHECKs name every hook the API is growing, so each one needs no rebuild of its own:
-- `tracksChanged` and `scanFinished` per library, `schedule` and `played` per plugin.

CREATE TABLE plugin_grants_hooks (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_grants_hooks SELECT plugin_id, permission FROM plugin_grants;
DROP TABLE plugin_grants;
ALTER TABLE plugin_grants_hooks RENAME TO plugin_grants;

CREATE TABLE plugin_library_grants_hooks (
    plugin_id  TEXT    NOT NULL,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    permission TEXT    NOT NULL CHECK (permission IN ('libraryRead', 'libraryAdd', 'libraryChange', 'tracksChanged', 'scanFinished')),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_library_grants_hooks SELECT plugin_id, library_id, permission FROM plugin_library_grants;
DROP TABLE plugin_library_grants;
ALTER TABLE plugin_library_grants_hooks RENAME TO plugin_library_grants;

-- How the plugin's last run in the library went, by hook or by Run now, for admins
-- (requirements/plugins.md §11).
ALTER TABLE plugin_libraries ADD COLUMN last_run_at INTEGER;
ALTER TABLE plugin_libraries ADD COLUMN last_run_ok INTEGER CHECK (last_run_ok IN (0, 1));
ALTER TABLE plugin_libraries ADD COLUMN last_run_summary TEXT;

-- How far each hook has delivered to a plugin in a library. Delivery resumes from here after
-- a failure or a restart, so nothing is missed (§8), and the position moves only once the
-- plugin has handled what came before it.
CREATE TABLE plugin_cursors (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished')),
    -- For tracksChanged, the last `library_changes.seq` delivered. Zero is before the first
    -- change, so a plugin new to a library works through all of it.
    position   INTEGER NOT NULL DEFAULT 0,
    -- Failures in a row, and when to try again after the latest.
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook)
) STRICT, WITHOUT ROWID;
