-- Idempotency for every signed-in write (design/database.md §9). The answer of a write that
-- applied is kept under the account's key for 8 days, longer than a device keeps a change to
-- send, and sent again to a retry. Rows stored by 0008_playlists.sql, for playlist creates and
-- edits, are JSON.
--
-- The answer's content type is kept so it is replayed as it was first sent; empty for an answer
-- with no body, such as a 204.
ALTER TABLE idempotency_keys ADD COLUMN content_type TEXT NOT NULL DEFAULT 'application/json';
