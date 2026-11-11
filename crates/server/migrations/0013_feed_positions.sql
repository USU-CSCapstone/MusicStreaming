-- Feed positions clients see (design/database.md §5).
--
-- `seq` numbers every library's changes in one sequence, and every account's in another, so its
-- jumps would tell a client about libraries and accounts it cannot reach (requirements/users.md
-- §10). It stays internal, for coalescing and the plugins' positions; a client sees `position`,
-- counted per library and per account from a head kept on the feed's owner. A head only grows,
-- so a client's cursor is never past it by a deletion.

ALTER TABLE libraries ADD COLUMN feed_head INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN feed_head INTEGER NOT NULL DEFAULT 0;
ALTER TABLE library_changes ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account_changes ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

UPDATE library_changes SET position = numbered.position
FROM (SELECT seq, row_number() OVER (PARTITION BY library_id ORDER BY seq) AS position
      FROM library_changes) AS numbered
WHERE numbered.seq = library_changes.seq;
UPDATE account_changes SET position = numbered.position
FROM (SELECT seq, row_number() OVER (PARTITION BY user_id ORDER BY seq) AS position
      FROM account_changes) AS numbered
WHERE numbered.seq = account_changes.seq;
UPDATE libraries SET feed_head =
    (SELECT coalesce(max(position), 0) FROM library_changes WHERE library_id = libraries.id);
UPDATE users SET feed_head =
    (SELECT coalesce(max(position), 0) FROM account_changes WHERE user_id = users.id);

DROP INDEX library_changes_feed;
CREATE INDEX library_changes_feed ON library_changes (library_id, position);
DROP INDEX account_changes_feed;
CREATE INDEX account_changes_feed ON account_changes (user_id, position);

-- Each start of the server is a run with an epoch of its own, which a client holds with its
-- position. A backup restored is started again, so a client of a run the backup never saw holds
-- an epoch this database does not know, and one of the run the backup was taken in may hold a
-- position the copy never reached: each start keeps every feed's head, which is where the run
-- before it ended.
CREATE TABLE feed_epochs (
    -- The order of the starts
    started INTEGER PRIMARY KEY,
    -- Random, so a run after a restore never takes the epoch of one the backup did not see
    epoch   INTEGER NOT NULL UNIQUE
) STRICT;

CREATE TABLE library_feed_starts (
    started    INTEGER NOT NULL REFERENCES feed_epochs (started),
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    head       INTEGER NOT NULL,
    PRIMARY KEY (library_id, started)
) STRICT, WITHOUT ROWID;

CREATE TABLE account_feed_starts (
    started INTEGER NOT NULL REFERENCES feed_epochs (started),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    head    INTEGER NOT NULL,
    PRIMARY KEY (user_id, started)
) STRICT, WITHOUT ROWID;
