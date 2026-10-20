-- Installed plugins, what the admin granted them, where they are enabled, and what they keep
-- (requirements/plugins.md). Each plugin's code is `state/plugins/<id>.wasm`.
--
-- Grants, settings, exemptions, and users' connections outlive an uninstall, so a reinstall
-- needs no re-approval and keeps its configuration (§5): they are keyed by the plugin's id, not
-- its row. What belongs to an installation goes with it.

CREATE TABLE plugins (
    id           TEXT    PRIMARY KEY,
    -- As the plugin file carries it (design/plugins.md §6), validated at install.
    manifest     TEXT    NOT NULL CHECK (json_valid(manifest)),
    -- Where it was installed from; null for an uploaded file.
    source_url   TEXT,
    installed_at INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

-- ───────────────────────────── Grants ─────────────────────────────

-- The permissions granted per plugin (§4.1). A hook is requested and approved exactly as a
-- permission is (§8), so the hooks granted per plugin are here too.
CREATE TABLE plugin_grants (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played', 'playing', 'searchActivity', 'searched', 'hostRateLimits')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;

-- The permissions and hooks granted per library. Write access is two permissions, granted
-- separately: adding files, and changing or deleting them.
CREATE TABLE plugin_library_grants (
    plugin_id  TEXT    NOT NULL,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    permission TEXT    NOT NULL CHECK (permission IN ('libraryRead', 'libraryAdd', 'libraryChange', 'tracksChanged', 'scanFinished', 'albumsChanged', 'artistsChanged')),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;

-- The hosts whose limits for every plugin an admin exempted a plugin from, lowercase
-- (`hostRateLimits`, design/plugins.md §7).
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
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule', 'played', 'playing', 'searched', 'albumsChanged', 'artistsChanged')),
    -- played, playing, and searched keep a position per user, so one user's failures never
    -- hold up another's. Every other hook has user 0.
    user_id    INTEGER NOT NULL DEFAULT 0,
    -- tracksChanged, albumsChanged, artistsChanged: the last `library_changes.seq` of their
    -- kind delivered. scanFinished: when the last scan delivered finished. schedule: when it
    -- last ran. played: the last `play_ends.seq` delivered. playing: the last
    -- `play_starts.seq` delivered, or passed over as stale. searched: the last
    -- `search_events.seq` delivered. Zero is before everything, so a plugin new to a library
    -- works through all of it.
    position   INTEGER NOT NULL DEFAULT 0,
    -- Failures in a row, and when to try again after the latest.
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
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

-- What each user sets for a plugin that acts for them, such as their own account on a
-- scrobbling service (§4.1, §6). Having them is being connected: a plugin receives a user's
-- listening only once they have connected it. They go with the user.
CREATE TABLE plugin_user_settings (
    plugin_id    TEXT    NOT NULL,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    settings     TEXT    NOT NULL CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    -- Listening from before this is never delivered: connecting is not a request to send
    -- everything ever played.
    connected_at INTEGER NOT NULL,
    PRIMARY KEY (plugin_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX plugin_user_settings_user ON plugin_user_settings (user_id);

-- Who shares their searches with which plugin, and since when (requirements/search.md §6). Off
-- unless a row is here, and only a connected user can turn it on.
CREATE TABLE plugin_search_sharing (
    plugin_id TEXT    NOT NULL,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    shared_at INTEGER NOT NULL,
    PRIMARY KEY (plugin_id, user_id)
) STRICT, WITHOUT ROWID;
