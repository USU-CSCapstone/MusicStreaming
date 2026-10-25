-- Catalog counts, most-used spellings and album groups, kept by ±1 as tracks and their links
-- change, so a scan batch costs what it changes rather than what the albums, artists and tags it
-- touches hold (design/database.md §3). The scanner turns the top spellings counted here into
-- names and sort keys, and resolves each album it touches from its groups. A count row goes when
-- its count reaches zero.

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

-- How many of the tracks crediting the artist first, as artist or as album artist, carry each
-- tracks.file_artist_sort_key or tracks.file_album_artist_sort_key. An empty file_album_artist_sort_key
-- is no vote: the album artist has no sort tag of its own to give.
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

-- ───────────────────────────── Album groups ─────────────────────────────

-- An album's tracks grouped by every value the album resolves from them, each group with how
-- many tracks it holds, how many of them are not missing, and their duration, so an album
-- resolves from its groups rather than from every track. Most albums have a group per disc. A
-- key can't hold NULL, so a value a track lacks is '' or -1 here, values the checks on tracks
-- keep any track from holding. labels and album_artist_ids copy the track's JSON arrays as
-- written, except that a track with no labels has ''.
CREATE TABLE album_groups (
    library_id      INTEGER NOT NULL,
    album_id        INTEGER NOT NULL,
    disc_number     INTEGER NOT NULL,
    title           TEXT    NOT NULL,
    sort_key        BLOB    NOT NULL,
    release_date    TEXT    NOT NULL,
    release_type    TEXT    NOT NULL,
    track_total     INTEGER NOT NULL,
    disc_total      INTEGER NOT NULL,
    compilation     INTEGER NOT NULL,
    labels          TEXT    NOT NULL,
    album_artist_ids TEXT    NOT NULL,
    track_count     INTEGER NOT NULL,
    available_count INTEGER NOT NULL,
    duration_us     INTEGER NOT NULL,
    PRIMARY KEY (album_id, disc_number, title, sort_key, release_date, release_type, track_total,
                 disc_total, compilation, labels, album_artist_ids),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TRIGGER album_groups_unused AFTER UPDATE OF track_count ON album_groups
WHEN NEW.track_count = 0
BEGIN
    DELETE FROM album_groups
    WHERE (album_id, disc_number, title, sort_key, release_date, release_type, track_total,
           disc_total, compilation, labels, album_artist_ids)
        = (NEW.album_id, NEW.disc_number, NEW.title, NEW.sort_key, NEW.release_date,
           NEW.release_type, NEW.track_total, NEW.disc_total, NEW.compilation, NEW.labels,
           NEW.album_artist_ids);
END;

-- ───────────────────────────── Tracks ─────────────────────────────

CREATE TRIGGER tracks_added AFTER INSERT ON tracks
BEGIN
    UPDATE libraries SET track_count = track_count + 1, duration_us = duration_us + NEW.duration_us,
        names_version = names_version + 1
    WHERE id = NEW.library_id;
    INSERT INTO album_groups VALUES (NEW.library_id, NEW.album_id, NEW.disc_number,
        ifnull(NEW.album_title, ''), NEW.file_album_sort_key, ifnull(NEW.release_date, ''),
        ifnull(NEW.release_type, ''), ifnull(NEW.track_total, -1), ifnull(NEW.disc_total, -1),
        NEW.compilation, ifnull(nullif(NEW.labels, '[]'), ''), NEW.album_artist_ids, 1,
        NEW.missing_since IS NULL, NEW.duration_us)
    ON CONFLICT DO UPDATE SET track_count = track_count + 1,
        available_count = available_count + excluded.available_count,
        duration_us = duration_us + excluded.duration_us;
END;

-- A track's artist credits, album artist credits and genres go before it, since the counts they
-- feed read the track.
CREATE TRIGGER tracks_removed BEFORE DELETE ON tracks
BEGIN
    DELETE FROM track_artists WHERE track_id = OLD.id;
    DELETE FROM track_album_artists WHERE track_id = OLD.id;
    DELETE FROM track_tags WHERE track_id = OLD.id;
    UPDATE libraries SET track_count = track_count - 1, duration_us = duration_us - OLD.duration_us,
        names_version = names_version + 1
    WHERE id = OLD.library_id;
    UPDATE album_groups SET track_count = track_count - 1,
        available_count = available_count - (OLD.missing_since IS NULL),
        duration_us = duration_us - OLD.duration_us
    WHERE (album_id, disc_number, title, sort_key, release_date, release_type, track_total,
           disc_total, compilation, labels, album_artist_ids)
        = (OLD.album_id, OLD.disc_number, ifnull(OLD.album_title, ''), OLD.file_album_sort_key,
           ifnull(OLD.release_date, ''), ifnull(OLD.release_type, ''), ifnull(OLD.track_total, -1),
           ifnull(OLD.disc_total, -1), OLD.compilation, ifnull(nullif(OLD.labels, '[]'), ''),
           OLD.album_artist_ids);
END;

-- A track leaves its old group for the one its new album, disc, tags, availability and duration
-- put it in.
CREATE TRIGGER tracks_regrouped AFTER UPDATE OF album_id, disc_number, album_title,
    file_album_sort_key, release_date, release_type, track_total, disc_total, compilation, labels,
    album_artist_ids, missing_since, duration_us
ON tracks
BEGIN
    UPDATE album_groups SET track_count = track_count - 1,
        available_count = available_count - (OLD.missing_since IS NULL),
        duration_us = duration_us - OLD.duration_us
    WHERE (album_id, disc_number, title, sort_key, release_date, release_type, track_total,
           disc_total, compilation, labels, album_artist_ids)
        = (OLD.album_id, OLD.disc_number, ifnull(OLD.album_title, ''), OLD.file_album_sort_key,
           ifnull(OLD.release_date, ''), ifnull(OLD.release_type, ''), ifnull(OLD.track_total, -1),
           ifnull(OLD.disc_total, -1), OLD.compilation, ifnull(nullif(OLD.labels, '[]'), ''),
           OLD.album_artist_ids);
    INSERT INTO album_groups VALUES (NEW.library_id, NEW.album_id, NEW.disc_number,
        ifnull(NEW.album_title, ''), NEW.file_album_sort_key, ifnull(NEW.release_date, ''),
        ifnull(NEW.release_type, ''), ifnull(NEW.track_total, -1), ifnull(NEW.disc_total, -1),
        NEW.compilation, ifnull(nullif(NEW.labels, '[]'), ''), NEW.album_artist_ids, 1,
        NEW.missing_since IS NULL, NEW.duration_us)
    ON CONFLICT DO UPDATE SET track_count = track_count + 1,
        available_count = available_count + excluded.available_count,
        duration_us = duration_us + excluded.duration_us;
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
    FROM track_artists c WHERE c.track_id = NEW.id AND artists.id = c.artist_id;
END;

-- The track's genres and credits move to its new album.
CREATE TRIGGER tracks_album AFTER UPDATE OF album_id ON tracks WHEN NEW.album_id <> OLD.album_id
BEGIN
    UPDATE album_tags SET track_count = track_count - 1
    WHERE album_id = OLD.album_id AND tag_id IN (SELECT tag_id FROM track_tags WHERE track_id = NEW.id);
    INSERT INTO album_tags (library_id, album_id, tag_id, track_count)
    SELECT library_id, NEW.album_id, tag_id, 1 FROM track_tags WHERE track_id = NEW.id
    ON CONFLICT (album_id, tag_id) DO UPDATE SET track_count = track_count + 1;
    UPDATE album_credits SET track_count = track_count - 1
    WHERE album_id = OLD.album_id
      AND artist_id IN (SELECT artist_id FROM track_artists WHERE track_id = NEW.id);
    INSERT INTO album_credits (library_id, album_id, artist_id, track_count)
    SELECT library_id, NEW.album_id, artist_id, 1 FROM track_artists WHERE track_id = NEW.id
    ON CONFLICT (album_id, artist_id) DO UPDATE SET track_count = track_count + 1;
END;

CREATE TRIGGER tracks_artist_sort_key AFTER UPDATE OF file_artist_sort_key ON tracks
WHEN NEW.file_artist_sort_key <> OLD.file_artist_sort_key
BEGIN
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE artist_id = (SELECT artist_id FROM track_artists WHERE track_id = NEW.id AND position = 0)
      AND sort_key = OLD.file_artist_sort_key;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT library_id, artist_id, NEW.file_artist_sort_key, 1 FROM track_artists
    WHERE track_id = NEW.id AND position = 0
    ON CONFLICT (artist_id, sort_key) DO UPDATE SET uses = uses + 1;
END;

CREATE TRIGGER tracks_album_artist_sort_key AFTER UPDATE OF file_album_artist_sort_key ON tracks
WHEN NEW.file_album_artist_sort_key <> OLD.file_album_artist_sort_key
BEGIN
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE OLD.file_album_artist_sort_key <> x''
      AND artist_id = (SELECT artist_id FROM track_album_artists WHERE track_id = NEW.id AND position = 0)
      AND sort_key = OLD.file_album_artist_sort_key;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT library_id, artist_id, NEW.file_album_artist_sort_key, 1 FROM track_album_artists
    WHERE track_id = NEW.id AND position = 0 AND NEW.file_album_artist_sort_key <> x''
    ON CONFLICT (artist_id, sort_key) DO UPDATE SET uses = uses + 1;
END;

-- A track copies its album's sort key and follows a change to it, which costs the album's tracks
-- and runs only when the key changes. Its first artist's key is followed by the artist's
-- recompute, in follow_artist_sort_key (design/database.md §3).
CREATE TRIGGER albums_sort_key AFTER UPDATE OF sort_key ON albums WHEN NEW.sort_key <> OLD.sort_key
BEGIN
    UPDATE tracks SET album_sort_key = NEW.sort_key WHERE album_id = NEW.id;
END;

-- ───────────────────────────── Track credits ─────────────────────────────

-- A credit counts toward the artist's tracks, available tracks while the track isn't missing,
-- its credits on the album, its spelling, the track's sort key when it is the first credit, and
-- each of the track's genres.
CREATE TRIGGER track_artists_added AFTER INSERT ON track_artists
BEGIN
    UPDATE artists
    SET track_count = track_count + 1,
        available_track_count = available_track_count
            + (SELECT count(*) FROM tracks WHERE id = NEW.track_id AND missing_since IS NULL)
    WHERE id = NEW.artist_id;
    INSERT INTO album_credits (library_id, album_id, artist_id, track_count)
    SELECT NEW.library_id, album_id, NEW.artist_id, 1 FROM tracks WHERE id = NEW.track_id
    ON CONFLICT (album_id, artist_id) DO UPDATE SET track_count = track_count + 1;
    INSERT INTO artist_spellings (library_id, artist_id, name, uses)
    SELECT NEW.library_id, NEW.artist_id, NEW.artist_name, 1 WHERE NEW.artist_name IS NOT NULL
    ON CONFLICT (artist_id, name) DO UPDATE SET uses = uses + 1;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT NEW.library_id, NEW.artist_id, file_artist_sort_key, 1 FROM tracks
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
            - (SELECT count(*) FROM tracks WHERE id = OLD.track_id AND missing_since IS NULL)
    WHERE id = OLD.artist_id;
    UPDATE album_credits SET track_count = track_count - 1
    WHERE album_id = (SELECT album_id FROM tracks WHERE id = OLD.track_id) AND artist_id = OLD.artist_id;
    UPDATE artist_spellings SET uses = uses - 1
    WHERE artist_id = OLD.artist_id AND name = OLD.artist_name;
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE OLD.position = 0 AND artist_id = OLD.artist_id
      AND sort_key = (SELECT file_artist_sort_key FROM tracks WHERE id = OLD.track_id);
    UPDATE artist_tags SET track_count = track_count - 1
    WHERE artist_id = OLD.artist_id AND tag_id IN (SELECT tag_id FROM track_tags WHERE track_id = OLD.track_id);
END;

CREATE TRIGGER track_album_artists_added AFTER INSERT ON track_album_artists
BEGIN
    INSERT INTO artist_spellings (library_id, artist_id, name, uses)
    SELECT NEW.library_id, NEW.artist_id, NEW.artist_name, 1 WHERE NEW.artist_name IS NOT NULL
    ON CONFLICT (artist_id, name) DO UPDATE SET uses = uses + 1;
    INSERT INTO artist_sort_keys (library_id, artist_id, sort_key, uses)
    SELECT NEW.library_id, NEW.artist_id, file_album_artist_sort_key, 1 FROM tracks
    WHERE id = NEW.track_id AND NEW.position = 0 AND file_album_artist_sort_key <> x''
    ON CONFLICT (artist_id, sort_key) DO UPDATE SET uses = uses + 1;
END;

CREATE TRIGGER track_album_artists_removed AFTER DELETE ON track_album_artists
BEGIN
    UPDATE artist_spellings SET uses = uses - 1
    WHERE artist_id = OLD.artist_id AND name = OLD.artist_name;
    UPDATE artist_sort_keys SET uses = uses - 1
    WHERE OLD.position = 0 AND artist_id = OLD.artist_id
      AND sort_key = (SELECT file_album_artist_sort_key FROM tracks WHERE id = OLD.track_id)
      AND sort_key <> x'';
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

-- An album an artist is listed on counts among its albums. Its owned and featured tracks and its
-- albums under any credit are read through album_credits when asked (design/database.md §3).
CREATE TRIGGER album_artists_added AFTER INSERT ON album_artists
BEGIN
    UPDATE artists SET album_count = album_count + 1 WHERE id = NEW.artist_id;
END;

CREATE TRIGGER album_artists_removed AFTER DELETE ON album_artists
BEGIN
    UPDATE artists SET album_count = album_count - 1 WHERE id = OLD.artist_id;
END;

CREATE TRIGGER album_credits_unused AFTER UPDATE OF track_count ON album_credits
WHEN NEW.track_count = 0
BEGIN
    DELETE FROM album_credits WHERE album_id = NEW.album_id AND artist_id = NEW.artist_id;
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

-- A library's names_version counts the changes to what search indexes: a track, album or artist
-- added or deleted (tracks_added, tracks_removed, and the album and artist triggers below), or
-- renamed.
CREATE TRIGGER tracks_renamed AFTER UPDATE OF title ON tracks WHEN NEW.title IS NOT OLD.title
BEGIN
    UPDATE libraries SET names_version = names_version + 1 WHERE id = NEW.library_id;
END;

CREATE TRIGGER albums_renamed AFTER UPDATE OF title ON albums WHEN NEW.title IS NOT OLD.title
BEGIN
    UPDATE libraries SET names_version = names_version + 1 WHERE id = NEW.library_id;
END;

CREATE TRIGGER artists_renamed AFTER UPDATE OF name ON artists WHEN NEW.name IS NOT OLD.name
BEGIN
    UPDATE libraries SET names_version = names_version + 1 WHERE id = NEW.library_id;
END;

CREATE TRIGGER albums_added AFTER INSERT ON albums
BEGIN
    UPDATE libraries SET album_count = album_count + 1, names_version = names_version + 1
    WHERE id = NEW.library_id;
END;

CREATE TRIGGER albums_removed AFTER DELETE ON albums
BEGIN
    UPDATE libraries SET album_count = album_count - 1, names_version = names_version + 1
    WHERE id = OLD.library_id;
END;

CREATE TRIGGER artists_added AFTER INSERT ON artists
BEGIN
    UPDATE libraries SET artist_count = artist_count + 1, names_version = names_version + 1
    WHERE id = NEW.library_id;
END;

CREATE TRIGGER artists_removed AFTER DELETE ON artists
BEGIN
    UPDATE libraries SET artist_count = artist_count - 1, names_version = names_version + 1
    WHERE id = OLD.library_id;
END;
