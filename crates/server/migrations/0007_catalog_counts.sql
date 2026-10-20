-- Catalog counts and most-used spellings, kept by ±1 as tracks and their links change, so a scan
-- batch costs what it changes rather than what the artists and tags it touches hold
-- (design/database.md §3). The scanner resolves each album it touches from its tracks, since a
-- maximum can't be kept by ±1, and turns the top spellings counted here into names and sort
-- keys. A count row goes when its count reaches zero.

-- ───────────────────────────── Spellings ─────────────────────────────

-- How many credits, in track_artists and track_album_artists, write the artist's name each way
CREATE TABLE artist_spellings (
    library_id INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    uses       INTEGER NOT NULL,
    PRIMARY KEY (artist_id, name),
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- How many of the tracks crediting the artist first carry each tracks.artist_sort_key
CREATE TABLE artist_sort_keys (
    library_id INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    sort_key   BLOB    NOT NULL,
    uses       INTEGER NOT NULL,
    PRIMARY KEY (artist_id, sort_key),
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- How many tracks write the tag each way
CREATE TABLE tag_spellings (
    library_id INTEGER NOT NULL,
    tag_id     INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    uses       INTEGER NOT NULL,
    PRIMARY KEY (tag_id, name),
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TRIGGER artist_spellings_unused AFTER UPDATE OF uses ON artist_spellings WHEN NEW.uses = 0
BEGIN
    DELETE FROM artist_spellings WHERE artist_id = NEW.artist_id AND name = NEW.name;
END;

CREATE TRIGGER artist_sort_keys_unused AFTER UPDATE OF uses ON artist_sort_keys WHEN NEW.uses = 0
BEGIN
    DELETE FROM artist_sort_keys WHERE artist_id = NEW.artist_id AND sort_key = NEW.sort_key;
END;

CREATE TRIGGER tag_spellings_unused AFTER UPDATE OF uses ON tag_spellings WHEN NEW.uses = 0
BEGIN
    DELETE FROM tag_spellings WHERE tag_id = NEW.tag_id AND name = NEW.name;
END;

-- ───────────────────────────── Tracks ─────────────────────────────

CREATE TRIGGER tracks_added AFTER INSERT ON tracks
BEGIN
    UPDATE libraries SET track_count = track_count + 1, duration_us = duration_us + NEW.duration_us
    WHERE id = NEW.library_id;
END;

-- A track's artist credits and genres go before it, since the counts they feed read the track.
CREATE TRIGGER tracks_removed BEFORE DELETE ON tracks
BEGIN
    DELETE FROM track_artists WHERE track_id = OLD.id;
    DELETE FROM track_tags WHERE track_id = OLD.id;
    UPDATE libraries SET track_count = track_count - 1, duration_us = duration_us - OLD.duration_us
    WHERE id = OLD.library_id;
END;

CREATE TRIGGER tracks_duration AFTER UPDATE OF duration_us ON tracks
WHEN NEW.duration_us <> OLD.duration_us
BEGIN
    UPDATE libraries SET duration_us = duration_us + NEW.duration_us - OLD.duration_us
    WHERE id = NEW.library_id;
END;

CREATE TRIGGER tracks_availability AFTER UPDATE OF missing_since ON tracks
WHEN (NEW.missing_since IS NULL) <> (OLD.missing_since IS NULL)
BEGIN
    UPDATE artists
    SET available_track_count = available_track_count + (NEW.missing_since IS NULL) - (OLD.missing_since IS NULL)
    WHERE id IN (SELECT artist_id FROM track_artists WHERE track_id = NEW.id);
END;

-- The track's genres move to its new album, and its artists' credits become or stop being
-- appearances, by whether each album lists them.
CREATE TRIGGER tracks_album AFTER UPDATE OF album_id ON tracks WHEN NEW.album_id <> OLD.album_id
BEGIN
    UPDATE album_tags SET track_count = track_count - 1
    WHERE album_id = OLD.album_id AND tag_id IN (SELECT tag_id FROM track_tags WHERE track_id = NEW.id);
    INSERT INTO album_tags (library_id, album_id, tag_id, track_count)
    SELECT library_id, NEW.album_id, tag_id, 1 FROM track_tags WHERE track_id = NEW.id
    ON CONFLICT (album_id, tag_id) DO UPDATE SET track_count = track_count + 1;
    UPDATE artists
    SET appearance_count = appearance_count
        + (NOT EXISTS (SELECT 1 FROM album_artists WHERE album_id = NEW.album_id AND artist_id = artists.id))
        - (NOT EXISTS (SELECT 1 FROM album_artists WHERE album_id = OLD.album_id AND artist_id = artists.id))
    WHERE id IN (SELECT artist_id FROM track_artists WHERE track_id = NEW.id);
END;

CREATE TRIGGER tracks_artist_sort_key AFTER UPDATE OF artist_sort_key ON tracks
WHEN NEW.artist_sort_key <> OLD.artist_sort_key
BEGIN
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE artist_id = (SELECT artist_id FROM track_artists WHERE track_id = NEW.id AND position = 0)
      AND sort_key = OLD.artist_sort_key;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT library_id, artist_id, NEW.artist_sort_key, 1 FROM track_artists
    WHERE track_id = NEW.id AND position = 0
    ON CONFLICT (artist_id, sort_key) DO UPDATE SET uses = uses + 1;
END;

-- ───────────────────────────── Track credits ─────────────────────────────

-- A credit counts toward the artist's tracks, available tracks while the track isn't missing,
-- appearances while the track's album doesn't list the artist, its spelling, the track's sort
-- key when it is the first credit, and each of the track's genres.
CREATE TRIGGER track_artists_added AFTER INSERT ON track_artists
BEGIN
    UPDATE artists
    SET track_count = track_count + 1,
        available_track_count = available_track_count
            + (SELECT count(*) FROM tracks WHERE id = NEW.track_id AND missing_since IS NULL),
        appearance_count = appearance_count + (SELECT count(*) FROM tracks t WHERE t.id = NEW.track_id
            AND NOT EXISTS (SELECT 1 FROM album_artists WHERE album_id = t.album_id AND artist_id = NEW.artist_id))
    WHERE id = NEW.artist_id;
    INSERT INTO artist_spellings (library_id, artist_id, name, uses)
    SELECT NEW.library_id, NEW.artist_id, NEW.artist_name, 1 WHERE NEW.artist_name IS NOT NULL
    ON CONFLICT (artist_id, name) DO UPDATE SET uses = uses + 1;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT NEW.library_id, NEW.artist_id, artist_sort_key, 1 FROM tracks
    WHERE id = NEW.track_id AND NEW.position = 0
    ON CONFLICT (artist_id, sort_key) DO UPDATE SET uses = uses + 1;
    INSERT INTO artist_tags (library_id, artist_id, tag_id, track_count)
    SELECT NEW.library_id, NEW.artist_id, tag_id, 1 FROM track_tags WHERE track_id = NEW.track_id
    ON CONFLICT (artist_id, tag_id) DO UPDATE SET track_count = track_count + 1;
END;

CREATE TRIGGER track_artists_removed AFTER DELETE ON track_artists
BEGIN
    UPDATE artists
    SET track_count = track_count - 1,
        available_track_count = available_track_count
            - (SELECT count(*) FROM tracks WHERE id = OLD.track_id AND missing_since IS NULL),
        appearance_count = appearance_count - (SELECT count(*) FROM tracks t WHERE t.id = OLD.track_id
            AND NOT EXISTS (SELECT 1 FROM album_artists WHERE album_id = t.album_id AND artist_id = OLD.artist_id))
    WHERE id = OLD.artist_id;
    UPDATE artist_spellings SET uses = uses - 1
    WHERE artist_id = OLD.artist_id AND name = OLD.artist_name;
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE OLD.position = 0 AND artist_id = OLD.artist_id
      AND sort_key = (SELECT artist_sort_key FROM tracks WHERE id = OLD.track_id);
    UPDATE artist_tags SET track_count = track_count - 1
    WHERE artist_id = OLD.artist_id AND tag_id IN (SELECT tag_id FROM track_tags WHERE track_id = OLD.track_id);
END;

CREATE TRIGGER track_album_artists_added AFTER INSERT ON track_album_artists
WHEN NEW.artist_name IS NOT NULL
BEGIN
    INSERT INTO artist_spellings (library_id, artist_id, name, uses)
    VALUES (NEW.library_id, NEW.artist_id, NEW.artist_name, 1)
    ON CONFLICT (artist_id, name) DO UPDATE SET uses = uses + 1;
END;

CREATE TRIGGER track_album_artists_removed AFTER DELETE ON track_album_artists
BEGIN
    UPDATE artist_spellings SET uses = uses - 1
    WHERE artist_id = OLD.artist_id AND name = OLD.artist_name;
END;

-- ───────────────────────────── Track genres ─────────────────────────────

CREATE TRIGGER track_tags_added AFTER INSERT ON track_tags
BEGIN
    UPDATE tags SET track_count = track_count + 1 WHERE id = NEW.tag_id;
    INSERT INTO tag_spellings (library_id, tag_id, name, uses)
    VALUES (NEW.library_id, NEW.tag_id, NEW.tag_name, 1)
    ON CONFLICT (tag_id, name) DO UPDATE SET uses = uses + 1;
    INSERT INTO album_tags (library_id, album_id, tag_id, track_count)
    SELECT NEW.library_id, album_id, NEW.tag_id, 1 FROM tracks WHERE id = NEW.track_id
    ON CONFLICT (album_id, tag_id) DO UPDATE SET track_count = track_count + 1;
    INSERT INTO artist_tags (library_id, artist_id, tag_id, track_count)
    SELECT NEW.library_id, artist_id, NEW.tag_id, 1 FROM track_artists WHERE track_id = NEW.track_id
    ON CONFLICT (artist_id, tag_id) DO UPDATE SET track_count = track_count + 1;
END;

CREATE TRIGGER track_tags_removed AFTER DELETE ON track_tags
BEGIN
    UPDATE tags SET track_count = track_count - 1 WHERE id = OLD.tag_id;
    UPDATE tag_spellings SET uses = uses - 1 WHERE tag_id = OLD.tag_id AND name = OLD.tag_name;
    UPDATE album_tags SET track_count = track_count - 1
    WHERE album_id = (SELECT album_id FROM tracks WHERE id = OLD.track_id) AND tag_id = OLD.tag_id;
    UPDATE artist_tags SET track_count = track_count - 1
    WHERE artist_id IN (SELECT artist_id FROM track_artists WHERE track_id = OLD.track_id)
      AND tag_id = OLD.tag_id;
END;

-- ───────────────────────────── Album and artist links ─────────────────────────────

-- The artist's credits on the album's tracks stop being appearances while the album lists them.
CREATE TRIGGER album_artists_added AFTER INSERT ON album_artists
BEGIN
    UPDATE artists
    SET album_count = album_count + 1,
        appearance_count = appearance_count - (SELECT count(*) FROM tracks t CROSS JOIN track_artists ta
            ON ta.track_id = t.id WHERE t.album_id = NEW.album_id AND ta.artist_id = NEW.artist_id)
    WHERE id = NEW.artist_id;
END;

CREATE TRIGGER album_artists_removed AFTER DELETE ON album_artists
BEGIN
    UPDATE artists
    SET album_count = album_count - 1,
        appearance_count = appearance_count + (SELECT count(*) FROM tracks t CROSS JOIN track_artists ta
            ON ta.track_id = t.id WHERE t.album_id = OLD.album_id AND ta.artist_id = OLD.artist_id)
    WHERE id = OLD.artist_id;
END;

CREATE TRIGGER album_tags_added AFTER INSERT ON album_tags
BEGIN
    UPDATE tags SET album_count = album_count + 1 WHERE id = NEW.tag_id;
END;

CREATE TRIGGER album_tags_unused AFTER UPDATE OF track_count ON album_tags WHEN NEW.track_count = 0
BEGIN
    DELETE FROM album_tags WHERE album_id = NEW.album_id AND tag_id = NEW.tag_id;
END;

CREATE TRIGGER album_tags_removed AFTER DELETE ON album_tags
BEGIN
    UPDATE tags SET album_count = album_count - 1 WHERE id = OLD.tag_id;
END;

CREATE TRIGGER artist_tags_added AFTER INSERT ON artist_tags
BEGIN
    UPDATE tags SET artist_count = artist_count + 1 WHERE id = NEW.tag_id;
END;

CREATE TRIGGER artist_tags_unused AFTER UPDATE OF track_count ON artist_tags WHEN NEW.track_count = 0
BEGIN
    DELETE FROM artist_tags WHERE artist_id = NEW.artist_id AND tag_id = NEW.tag_id;
END;

CREATE TRIGGER artist_tags_removed AFTER DELETE ON artist_tags
BEGIN
    UPDATE tags SET artist_count = artist_count - 1 WHERE id = OLD.tag_id;
END;

-- ───────────────────────────── Library totals ─────────────────────────────

CREATE TRIGGER albums_added AFTER INSERT ON albums
BEGIN
    UPDATE libraries SET album_count = album_count + 1 WHERE id = NEW.library_id;
END;

CREATE TRIGGER albums_removed AFTER DELETE ON albums
BEGIN
    UPDATE libraries SET album_count = album_count - 1 WHERE id = OLD.library_id;
END;

CREATE TRIGGER artists_added AFTER INSERT ON artists
BEGIN
    UPDATE libraries SET artist_count = artist_count + 1 WHERE id = NEW.library_id;
END;

CREATE TRIGGER artists_removed AFTER DELETE ON artists
BEGIN
    UPDATE libraries SET artist_count = artist_count - 1 WHERE id = OLD.library_id;
END;
