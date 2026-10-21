-- Installed plugins, what the admin granted them, where they are enabled, and what they keep
-- (requirements/plugins.md). Each plugin's code is `state/plugins/<id>.wasm`.
--
-- Grants, settings, and exemptions outlive an uninstall, so a reinstall needs no re-approval and
-- keeps its configuration (§5): they are keyed by the plugin's id, not its row. What belongs to
-- an installation goes with it, users' connections included.

CREATE TABLE plugins (
    id           TEXT    PRIMARY KEY,
    -- As the plugin file carries it (design/plugins.md §6), validated at install.
    manifest     TEXT    NOT NULL CHECK (json_valid(manifest)),
    -- Where it was installed from; null for an uploaded file.
    source_url   TEXT,
    installed_at INTEGER NOT NULL
) STRICT;

-- ───────────────────────────── Grants ─────────────────────────────

-- The permissions granted per plugin (§4.1), in each library and to the plugin itself. A hook
-- is requested and approved exactly as a permission is (§8), so the hooks granted are here too.
-- Write access is two permissions, granted separately: adding files, and changing or deleting
-- them. Names are checked in Rust, which reads every manifest and grant, so a new permission or
-- hook needs no migration.
CREATE TABLE plugin_grants (
    plugin_id  TEXT    NOT NULL,
    -- The library a per-library permission is granted in, or zero for a permission granted to
    -- the plugin itself, which holds in every library.
    library_id INTEGER NOT NULL DEFAULT 0,
    permission TEXT    NOT NULL,
    -- The manifest's request the admin approved, in the server's canonical JSON. A reinstall
    -- keeps the grant only if its manifest makes exactly this request again.
    approved   TEXT    NOT NULL CHECK (json_valid(approved) AND json_type(approved) = 'object'),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;

-- The hosts whose limits for every plugin an admin exempted a plugin from, lowercase
-- (`hostRateLimits`, design/plugins.md §7). A reinstall keeps them only with its `network` grant.
CREATE TABLE plugin_rate_limit_exemptions (
    plugin_id TEXT NOT NULL,
    host      TEXT NOT NULL,
    PRIMARY KEY (plugin_id, host)
) STRICT, WITHOUT ROWID;

-- ───────────────────────────── Libraries ─────────────────────────────

-- Enabled per library, independently (§5). No row is disabled.
CREATE TABLE plugin_libraries (
    plugin_id        TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id       INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    enabled          INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    -- Why the server disabled it, such as a required permission revoked; null if the admin did.
    disabled_reason  TEXT,
    -- How the plugin's last run in the library went, by hook or by Run now, for admins (§11).
    last_run_at      INTEGER,
    last_run_ok      INTEGER CHECK (last_run_ok IN (0, 1)),
    last_run_summary TEXT,
    PRIMARY KEY (plugin_id, library_id)
) STRICT, WITHOUT ROWID;

-- What a plugin keeps between runs, per library (§2.1): its own keys and values, which only it
-- reads.
CREATE TABLE plugin_state (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      BLOB    NOT NULL,
    PRIMARY KEY (plugin_id, library_id, key)
) STRICT, WITHOUT ROWID;

-- How far each hook has delivered to a plugin in a library. Delivery resumes from here after
-- a failure or a restart, so nothing is missed (§8), and the position moves only once the
-- plugin has handled what came before it.
CREATE TABLE plugin_cursors (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL,
    -- played, playing, and searched keep a position per user, so one user's failures never
    -- hold up another's. Every other hook has user 0.
    user_id    INTEGER NOT NULL DEFAULT 0,
    -- tracksChanged, albumsChanged, artistsChanged: the last `library_changes.seq` of their
    -- kind delivered. scanFinished: when the last scan delivered finished. schedule: when it
    -- last ran. played: the last `play_events.seq` of an end delivered. playing: the last
    -- `play_events.seq` of a start delivered, or passed over as stale. searched: the last
    -- `search_events.seq` delivered. Zero is before everything, so a plugin new to a library
    -- works through all of it.
    position   INTEGER NOT NULL DEFAULT 0,
    -- Failures in a row, and when to try again after the latest. `outages` are the failures
    -- after the plugin found a service out of reach, and `failures` the rest; the two together
    -- set the backoff. `outage_since` is when the outages since the last success or other
    -- failure began, so an outage that has lasted too long can be told apart.
    failures     INTEGER NOT NULL DEFAULT 0,
    outages      INTEGER NOT NULL DEFAULT 0,
    outage_since INTEGER,
    retry_at     INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook, user_id)
) STRICT, WITHOUT ROWID;

-- ───────────────────────────── Settings ─────────────────────────────

-- What admins set for each plugin, as its manifest declares (§6): server-wide, and per library
-- where a library's differ. Secrets are kept here to be used, and the API never returns them.
CREATE TABLE plugin_settings (
    plugin_id  TEXT    NOT NULL,
    -- Zero for the settings of every library; a library's own override them.
    library_id INTEGER NOT NULL DEFAULT 0,
    settings   TEXT    NOT NULL CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    PRIMARY KEY (plugin_id, library_id)
) STRICT, WITHOUT ROWID;

-- Grants and settings keep what applies in every library as library 0, which no library has,
-- so they cannot reference `libraries` and a library's rows are deleted here instead. Recreate
-- this trigger whenever `libraries` is rebuilt.
CREATE TRIGGER libraries_delete_plugin_rows AFTER DELETE ON libraries
BEGIN
    DELETE FROM plugin_grants WHERE library_id = OLD.id;
    DELETE FROM plugin_settings WHERE library_id = OLD.id;
END;

-- Each user's connection to a plugin that acts for them, such as with their own account on a
-- scrobbling service (§4.1, §6), the same in every library. Having a row is being connected,
-- and a plugin receives a user's listening only once they have connected it. They go with the
-- user, and with the plugin: after a reinstall users connect again, so nothing they did before is
-- sent twice.
CREATE TABLE plugin_connections (
    plugin_id          TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    user_id            INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Their personal settings, secrets included, which the API never returns.
    settings           TEXT    NOT NULL CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    -- Listening from before this is never delivered: connecting is not a request to send
    -- everything ever played.
    connected_at       INTEGER NOT NULL,
    -- Since when they share their searches with it, in every library (requirements/search.md
    -- §6); null until they turn that on. Sharing is part of the connection, so it ends with it.
    searches_shared_at INTEGER,
    PRIMARY KEY (plugin_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX plugin_connections_by_user ON plugin_connections (user_id);
