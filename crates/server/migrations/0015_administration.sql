-- Account administration (requirements/users.md): accounts deleted in stages, password reset
-- links, invites, and the record of what admins did to accounts.
--
-- SQLite cannot change a CHECK or drop a NOT NULL, so `users` is made again, keeping every row.
-- The new table is renamed into place after the old one is dropped: renaming `users` itself
-- would rewrite every reference to it to name the old table.

CREATE TABLE users_new (
    id                INTEGER PRIMARY KEY,
    -- NULL while the account is being deleted, which frees the name at once
    username          TEXT    UNIQUE COLLATE NOCASE CHECK (length(username) BETWEEN 1 AND 32 AND username NOT GLOB '*[^A-Za-z0-9_-]*'),
    display_name      TEXT    NOT NULL,
    role              TEXT    NOT NULL CHECK (role IN ('owner', 'admin', 'user')),
    -- 'deleting' while the account's rows are emptied in chunks, before the row itself goes:
    -- every reader takes it as gone
    status            TEXT    NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'deleting')),
    password          TEXT    NOT NULL,
    avatar_hash       BLOB,
    created_at        INTEGER NOT NULL,
    feed_head         INTEGER NOT NULL DEFAULT 0,
    -- The one live reset link: its code's hash, and when it stops working
    reset_hash        BLOB    UNIQUE,
    reset_expires_at  INTEGER,
    -- The owner can be neither suspended nor deleted (requirements/users.md §1).
    CHECK (NOT (role = 'owner' AND status <> 'active')),
    CHECK ((username IS NULL) = (status = 'deleting')),
    CHECK ((reset_hash IS NULL) = (reset_expires_at IS NULL))
) STRICT;

INSERT INTO users_new (id, username, display_name, role, status, password, avatar_hash, created_at,
                       feed_head)
SELECT id, username, display_name, role, status, password, avatar_hash, created_at, feed_head
FROM users;

DROP TABLE users;
ALTER TABLE users_new RENAME TO users;

-- At most one owner
CREATE UNIQUE INDEX users_one_owner ON users (role) WHERE role = 'owner';

-- A single-use link that makes an account with the libraries it names. Whether it is pending,
-- redeemed, revoked or expired follows from its times, so it is never stored.
CREATE TABLE invites (
    id           INTEGER PRIMARY KEY,
    code_hash    BLOB    NOT NULL UNIQUE,
    created_by   INTEGER REFERENCES users (id) ON DELETE SET NULL,
    redeemed_by  INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    redeemed_at  INTEGER,
    revoked_at   INTEGER
) STRICT;

-- Newest first, for the list of invites
CREATE INDEX invites_by_created ON invites (created_at, id);
-- So deleting an account finds its invites without reading them all
CREATE INDEX invites_by_creator ON invites (created_by) WHERE created_by IS NOT NULL;
CREATE INDEX invites_by_redeemer ON invites (redeemed_by) WHERE redeemed_by IS NOT NULL;

-- The libraries an invite grants
CREATE TABLE invite_libraries (
    invite_id   INTEGER NOT NULL REFERENCES invites (id) ON DELETE CASCADE,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    PRIMARY KEY (invite_id, library_id)
) STRICT, WITHOUT ROWID;

-- What was done to each account, and by whom (requirements/users.md §8). No foreign keys: a row
-- outlives both accounts, so it keeps their names as they were when it was written. The actor
-- is NULL for what no one signed in did, such as the host's owner reset.
CREATE TABLE admin_actions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    at           INTEGER NOT NULL,
    action       TEXT    NOT NULL CHECK (action IN ('account_created', 'access_changed', 'suspended',
                     'restored', 'devices_signed_out', 'password_reset_issued',
                     'password_reset_used', 'role_changed', 'ownership_transferred', 'deleted')),
    actor_id     INTEGER,
    actor_name   TEXT,
    target_id    INTEGER NOT NULL,
    target_name  TEXT    NOT NULL,
    detail       TEXT    NOT NULL CHECK (json_valid(detail) AND json_type(detail) = 'object'),
    CHECK ((actor_id IS NULL) = (actor_name IS NULL))
) STRICT;

-- So deleting an account finds its rows here without scanning these tables, whose other indexes
-- lead with the library (design/database.md §7)
CREATE INDEX login_attempts_by_user ON login_attempts (user_id) WHERE user_id IS NOT NULL;
CREATE INDEX mark_events_by_user ON mark_events (user_id);
CREATE INDEX search_events_by_user ON search_events (user_id);
