-- Browsing tracks by duration (`requirements/conventions.md` §4), as `albums_by_duration` does
-- albums (`0003_catalog.sql`).

CREATE INDEX tracks_by_duration ON tracks (library_id, duration_us, id);
