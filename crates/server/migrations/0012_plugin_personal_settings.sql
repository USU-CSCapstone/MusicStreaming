-- What each user sets for a plugin that acts for them, such as their own account on a
-- scrobbling service (requirements/plugins.md §4.1, §6). Having them is being connected: a
-- plugin receives a user's listening only once they have connected it. Like an admin's
-- settings, they are keyed by the plugin's id and survive a reinstall; they go with the user.

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
