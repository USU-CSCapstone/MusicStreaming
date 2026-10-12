-- `globalRateLimits`: letting a plugin's rate limits that say "per": "global" be shared with
-- other plugins (design/plugins.md §7). Granted per plugin, like `network`.

CREATE TABLE plugin_grants_global (
    plugin_id  TEXT NOT NULL,
    permission TEXT NOT NULL CHECK (permission IN ('network', 'listeningActivity', 'schedule', 'played', 'globalRateLimits')),
    PRIMARY KEY (plugin_id, permission)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_grants_global SELECT plugin_id, permission FROM plugin_grants;
DROP TABLE plugin_grants;
ALTER TABLE plugin_grants_global RENAME TO plugin_grants;
