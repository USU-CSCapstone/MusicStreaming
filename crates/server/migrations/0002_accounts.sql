-- Accounts, library access, devices and their sessions, invites, password resets, and login attempts (requirements/users.md).

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (length(username) BETWEEN 1 AND 32 AND username NOT GLOB '*[^A-Za-z0-9_-]*'),
    display_name  TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('owner', 'admin', 'user')),
    status        TEXT    NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    password      TEXT    NOT NULL,
    avatar_hash   BLOB,
    -- Oldest account-feed position still answerable (see 0004).
    feed_horizon  INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    -- The owner cannot be suspended (requirements/users.md §1).
    CHECK (NOT (role = 'owner' AND status = 'suspended'))
) STRICT;

-- At most one owner
CREATE UNIQUE INDEX users_one_owner ON users (role) WHERE role = 'owner';

CREATE TABLE user_settings (
    user_id            INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    default_library_id INTEGER REFERENCES libraries (id) ON DELETE SET NULL,
    settings           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    updated_at         INTEGER NOT NULL
) STRICT;

-- Explicit grants. Admins and the owner reach every library without a row here.
CREATE TABLE library_access (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    granted_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, library_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX library_access_library ON library_access (library_id);

-- A device is a session: every login registers one (requirements/users.md §4).
CREATE TABLE devices (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name           TEXT    NOT NULL,
    type           TEXT    NOT NULL CHECK (type IN ('phone', 'tablet', 'desktop', 'tv')),
    platform       TEXT,
    token_hash     BLOB    UNIQUE,
    revoked_reason TEXT    CHECK (revoked_reason IN ('credentials_changed')),
    created_at     INTEGER NOT NULL,
    last_seen_at   INTEGER NOT NULL,
    CHECK ((token_hash IS NULL) = (revoked_reason IS NOT NULL))
) STRICT;

CREATE INDEX devices_user ON devices (user_id);

CREATE TABLE invites (
    id         INTEGER PRIMARY KEY,
    code_hash  BLOB    NOT NULL UNIQUE,
    created_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    max_uses   INTEGER NOT NULL DEFAULT 1 CHECK (max_uses >= 1),
    uses       INTEGER NOT NULL DEFAULT 0,
    revoked_at INTEGER,
    CHECK (uses BETWEEN 0 AND max_uses)
) STRICT;

-- Access new accounts from an invite start with.
CREATE TABLE invite_libraries (
    invite_id  INTEGER NOT NULL REFERENCES invites (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    PRIMARY KEY (invite_id, library_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE password_resets (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash  BLOB    NOT NULL UNIQUE,
    created_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER
) STRICT;

CREATE INDEX password_resets_user ON password_resets (user_id);

-- Every attempt, for admins (requirements/users.md §3.1).
CREATE TABLE login_attempts (
    id                 INTEGER PRIMARY KEY,
    at                 INTEGER NOT NULL,
    username_attempted TEXT    NOT NULL COLLATE NOCASE,
    user_id            INTEGER REFERENCES users (id) ON DELETE SET NULL,
    origin             TEXT    NOT NULL,
    succeeded          INTEGER NOT NULL CHECK (succeeded IN (0, 1)),
    locked_out         INTEGER NOT NULL DEFAULT 0 CHECK (locked_out IN (0, 1))
) STRICT;

-- Per-account limits count by the name tried, so unknown usernames lock out exactly as real ones do.
CREATE INDEX login_attempts_username ON login_attempts (username_attempted, at);
CREATE INDEX login_attempts_origin ON login_attempts (origin, at);
CREATE INDEX login_attempts_at ON login_attempts (at);
