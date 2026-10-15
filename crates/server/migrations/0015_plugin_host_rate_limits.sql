-- `hostRateLimits`: letting a plugin set a rate limit that every plugin's requests to a host
-- keep to (design/plugins.md §7). Granted per plugin, like `network`.

CREATE TABLE plugin_grants_host (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played', 'hostRateLimits')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_grants_host SELECT plugin_id, permission FROM plugin_grants;
DROP TABLE plugin_grants;
ALTER TABLE plugin_grants_host RENAME TO plugin_grants;

-- The hosts whose limits for every plugin an admin exempted a plugin from, lowercase. Kept by
-- the plugin's id, like its grants, so a reinstall keeps them (requirements/plugins.md §5).
CREATE TABLE plugin_rate_limit_exemptions (
    plugin_id TEXT NOT NULL,
    host      TEXT NOT NULL,
    PRIMARY KEY (plugin_id, host)
) STRICT, WITHOUT ROWID;
