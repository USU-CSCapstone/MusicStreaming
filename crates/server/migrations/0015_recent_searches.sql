-- Recent searches (requirements/search.md §6): what the search box offers when it is empty.
-- Each user's own, per library, and kept only for a recent window (`api/recent_searches.rs`),
-- so there is no permanent search history.

CREATE TABLE recent_searches (
    -- Never reused, so an ID names one search even after the same words are searched again.
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    query         TEXT    NOT NULL,
    -- The query's words as search compares them, ignoring case and accents. Searching the same
    -- words again replaces the entry.
    folded        TEXT    NOT NULL,
    -- The result the user acted on, if any.
    selected_type TEXT    CHECK (selected_type IN ('track', 'album', 'artist')),
    selected_id   INTEGER,
    searched_at   INTEGER NOT NULL,
    CHECK ((selected_type IS NULL) = (selected_id IS NULL))
) STRICT;

CREATE UNIQUE INDEX recent_searches_query ON recent_searches (user_id, library_id, folded);
CREATE INDEX recent_searches_newest ON recent_searches (user_id, library_id, searched_at);
-- For sweeping away everyone's searches once they age out of the window.
CREATE INDEX recent_searches_age ON recent_searches (searched_at);
