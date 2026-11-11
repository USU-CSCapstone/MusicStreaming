-- What plugins can see of the library change feed (design/hooks.md §1.3, design/database.md §5).
--
-- A change row also carries the position of the last change to its entity that plugins could
-- see, so a change they cannot see (an image's placeholder, a copied sort key, loudness) moves
-- the entity for devices without waking the tracksChanged, albumsChanged and artistsChanged
-- hooks. plugin_view fingerprints what the plugin API reads of the entity as of the change, NULL
-- for a deletion and for a kind no hook reads. plugin_since is the position plugins last saw a
-- change at, kept when this change left the fingerprint as it was; NULL means this row's own seq.

ALTER TABLE library_changes ADD COLUMN plugin_view  INTEGER;
ALTER TABLE library_changes ADD COLUMN plugin_since INTEGER;
ALTER TABLE library_changes ADD COLUMN plugin_seq   INTEGER
    GENERATED ALWAYS AS (coalesce(plugin_since, seq)) VIRTUAL;

-- Each change hook reads one kind from its own position, as library_changes_kind_feed let it.
DROP INDEX library_changes_kind_feed;
CREATE INDEX library_changes_plugin_feed ON library_changes (library_id, entity_type, plugin_seq);
