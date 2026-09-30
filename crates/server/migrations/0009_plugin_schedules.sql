-- A schedule keeps its place like the other hooks: `position` is when it last ran.

CREATE TABLE plugin_cursors_schedule (
    plugin_id  TEXT    NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hook       TEXT    NOT NULL CHECK (hook IN ('tracksChanged', 'scanFinished', 'schedule')),
    -- tracksChanged: the last `library_changes.seq` delivered. scanFinished: when the last
    -- scan delivered finished. schedule: when it last ran. Zero is before everything.
    position   INTEGER NOT NULL DEFAULT 0,
    failures   INTEGER NOT NULL DEFAULT 0,
    retry_at   INTEGER,
    PRIMARY KEY (plugin_id, library_id, hook)
) STRICT, WITHOUT ROWID;
INSERT INTO plugin_cursors_schedule SELECT * FROM plugin_cursors;
DROP TABLE plugin_cursors;
ALTER TABLE plugin_cursors_schedule RENAME TO plugin_cursors;
