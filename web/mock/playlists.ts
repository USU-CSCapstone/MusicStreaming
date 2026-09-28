// Sample playlists. There is no playlist storage yet, so the mock derives a few
// deterministic ones from the library — enough to build and see the Playlists views.

import type { Playlist } from '../src/lib/api/types.ts';
import { type Db, iso, num } from './db.ts';

export type MockPlaylist = { playlist: Playlist; trackIds: bigint[] };

type Recipe = { id: string; title: string; description: string; sql: string; params?: bigint[] };

const CACHE_MS = 10_000;
const cache = new Map<bigint, { at: number; lists: MockPlaylist[] }>();

export function playlists(db: Db, lib: bigint): MockPlaylist[] {
	const hit = cache.get(lib);
	if (hit && Date.now() - hit.at < CACHE_MS) return hit.lists;
	const lists = build(db, lib);
	cache.set(lib, { at: Date.now(), lists });
	return lists;
}

export function playlist(db: Db, lib: bigint, playlistId: string): MockPlaylist | undefined {
	return playlists(db, lib).find((p) => p.playlist.id === playlistId);
}

function build(db: Db, lib: bigint): MockPlaylist[] {
	const recipes: Recipe[] = [
		{
			id: 'pl-recent',
			title: 'Recently Added',
			description: 'The newest arrivals in your library.',
			sql: 'SELECT id FROM tracks WHERE library_id = ? AND missing_since IS NULL ORDER BY added_at DESC, id LIMIT 25'
		},
		{
			id: 'pl-long',
			title: 'Long Listens',
			description: 'The longest tracks you have.',
			sql: 'SELECT id FROM tracks WHERE library_id = ? AND missing_since IS NULL ORDER BY duration_us DESC, id LIMIT 25'
		}
	];
	const genres = db.all(
		"SELECT id, name FROM tags WHERE library_id = ? AND type = 'genre' ORDER BY track_count DESC, sort_key LIMIT 2",
		lib
	);
	for (const g of genres) {
		recipes.push({
			id: `pl-genre-${g.id}`,
			title: `${g.name} Mix`,
			description: `A run through your ${g.name} collection.`,
			sql: `SELECT t.id FROM track_tags x JOIN tracks t ON t.id = x.track_id
			      WHERE t.library_id = ? AND x.tag_id = ? ORDER BY t.album_sort_key, t.disc_number, t.track_number, t.id LIMIT 50`,
			params: [g.id as bigint]
		});
	}

	const created = iso(
		db.get('SELECT MIN(created_at) AS at FROM library_roots WHERE library_id = ?', lib)?.at ?? 0
	);
	return recipes.flatMap((r) => {
		const trackIds = db.all(r.sql, lib, ...(r.params ?? [])).map((row) => row.id as bigint);
		if (trackIds.length === 0) return [];
		const totals = db.get(
			`SELECT SUM(duration_us) AS us, SUM(missing_since IS NOT NULL) AS missing
			 FROM tracks WHERE id IN (${trackIds.map(() => '?').join(', ')})`,
			...trackIds
		);
		return [
			{
				trackIds,
				playlist: {
					id: r.id,
					libraryId: String(lib),
					title: r.title,
					description: r.description,
					// A mosaic of the albums inside (`requirements/playlists.md` §6), composed by `media.ts`.
					image: { id: `mosaic-${r.id}`, placeholder: '' },
					customImage: false,
					trackCount: trackIds.length,
					missingCount: num(totals?.missing ?? 0),
					durationUs: num(totals?.us ?? 0),
					pinned: false,
					createdAt: created,
					modifiedAt: created,
					version: 1
				}
			}
		];
	});
}
