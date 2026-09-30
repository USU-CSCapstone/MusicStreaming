-- Write access to a library is two permissions, granted separately (requirements/plugins.md
-- §4.1): adding files, and changing or deleting them. A plugin granted write access keeps
-- both halves of it.

CREATE TABLE plugin_library_grants_split (
    plugin_id  TEXT    NOT NULL,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    permission TEXT    NOT NULL CHECK (permission IN ('libraryRead', 'libraryAdd', 'libraryChange')),
    PRIMARY KEY (plugin_id, library_id, permission)
) STRICT, WITHOUT ROWID;

INSERT INTO plugin_library_grants_split (plugin_id, library_id, permission)
SELECT plugin_id, library_id, permission FROM plugin_library_grants WHERE permission = 'libraryRead'
UNION
SELECT plugin_id, library_id, split.value FROM plugin_library_grants,
       json_each('["libraryAdd", "libraryChange"]') AS split
WHERE permission = 'libraryWrite';

DROP TABLE plugin_library_grants;
ALTER TABLE plugin_library_grants_split RENAME TO plugin_library_grants;

-- Manifests stored before the split ask for write access by its old name.
UPDATE plugins SET manifest = replace(manifest, '"permission":"libraryWrite"', '"permission":"libraryAdd"');
