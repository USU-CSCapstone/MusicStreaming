import { beforeEach, describe, expect, it } from 'vitest';
import type {
	Playlist,
	PlaylistItemPage,
	PlaylistPage,
	Problem,
	TrackSummary
} from '../src/lib/api/types';
import { type PlaylistLibrary, PlaylistStore, playlistRoute } from './playlists';

// Library 1 has tracks 1–6: album 100 is tracks 1–3, artist 200 has tracks 4 and 5. Library 2
// has track 7 alone, so nothing of library 1 may reach it.
const LIB = 1n;
const OTHER = 2n;

/** The part of the request body a `validation_failed` problem's `detail` points at. */
const field = (problem: Problem) => /\(at (.*)\)$/.exec(problem.detail ?? '')?.[1] ?? '';

const track = (n: number): TrackSummary => ({
	id: String(n),
	title: `Track ${n}`,
	artists: [],
	album: { id: '100', title: 'Album', artists: [], image: null },
	discNumber: 1,
	trackNumber: n,
	durationUs: n * 1_000_000,
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available'
});

const contents: Record<string, Record<string, number[]>> = {
	'1': { track: [1, 2, 3, 4, 5, 6], album100: [1, 2, 3], artist200: [4, 5] },
	'2': { track: [7] }
};

const library: PlaylistLibrary = {
	generated: (lib) =>
		lib === LIB
			? [
					{
						id: 'pl-seed',
						title: 'Seed',
						description: 'Generated.',
						createdAt: '2026-01-01T00:00:00.000Z',
						trackIds: [1n, 2n]
					}
				]
			: [],
	expand(lib, ref) {
		const have = contents[String(lib)];
		if (ref.type === 'track') {
			return have.track.includes(Number(ref.id)) ? [BigInt(ref.id)] : undefined;
		}
		return have[`${ref.type}${ref.id}`]?.map(BigInt);
	},
	track: (lib, id) =>
		contents[String(lib)].track.includes(Number(id)) ? track(Number(id)) : undefined
};

let store: PlaylistStore;
beforeEach(() => {
	store = new PlaylistStore(library);
});

function call<T>(method: string, path: string, body?: unknown, key?: string, lib = LIB) {
	const bytes = new TextEncoder().encode(body === undefined ? '' : JSON.stringify(body));
	const out = playlistRoute(store, lib, method, path, bytes, key);
	if (!out) throw new Error(`no route for ${path}`);
	return out as { status: number; body: T };
}

const ids = (path: string, lib = LIB) =>
	call<PlaylistItemPage>('GET', `${path}/items`, undefined, undefined, lib).body.items.map(
		(i) => i.track?.id
	);

function create(body: object = { title: 'Mine' }, key?: string) {
	const out = call<Playlist>('POST', '/playlists', body, key);
	expect(out.status).toBe(201);
	return out.body;
}

describe('reading', () => {
	it('starts with the generated playlists', () => {
		const page = call<PlaylistPage>('GET', '/playlists').body;
		expect(page.items.map((p) => p.id)).toEqual(['liked-1', 'pl-seed']);
		expect(page.items[0]).toMatchObject({ kind: 'liked', pinned: true, trackCount: 0 });
		expect(page.items[1]).toMatchObject({ version: 1, trackCount: 2, durationUs: 3_000_000 });
		expect(ids('/playlists/pl-seed')).toEqual(['1', '2']);
	});

	it('pages items with the version their positions belong to', () => {
		const items = call<PlaylistItemPage>('GET', '/playlists/pl-seed/items?limit=1').body;
		expect(items.items.map((i) => i.track?.id)).toEqual(['1']);
		expect(items.version).toBe(1);
		expect(items.nextCursor).not.toBeNull();
	});

	it('answers 404 for a playlist that does not exist', () => {
		expect(call('GET', '/playlists/nope').status).toBe(404);
		expect(call('GET', '/playlists/nope/items').status).toBe(404);
	});

	it('lists pinned playlists first, and filters by name', () => {
		const mine = create({ title: 'Road Trip' });
		call('PATCH', `/playlists/${mine.id}`, { baseVersion: 1, pinned: true });
		const list = (q = '') =>
			call<PlaylistPage>('GET', `/playlists${q}`).body.items.map((p) => p.title);
		expect(list()).toEqual(['Liked Songs', 'Road Trip', 'Seed']);
		expect(list('?q=road')).toEqual(['Road Trip']);
	});

	it('keeps each library to its own playlists', () => {
		const other = call<PlaylistPage>('GET', '/playlists', undefined, undefined, OTHER).body;
		expect(other.items.map((p) => p.id)).toEqual(['liked-2']);
		expect(call('GET', '/playlists/pl-seed', undefined, undefined, OTHER).status).toBe(404);
	});
});

describe('creating', () => {
	it('creates at version 1, with albums expanded to their tracks', () => {
		const made = create({
			title: 'Mine',
			description: 'Mixed.',
			items: [
				{ type: 'track', id: '6' },
				{ type: 'album', id: '100' }
			]
		});
		expect(made).toMatchObject({
			title: 'Mine',
			description: 'Mixed.',
			version: 1,
			trackCount: 4,
			durationUs: 12_000_000,
			pinned: false,
			libraryId: '1'
		});
		expect(ids(`/playlists/${made.id}`)).toEqual(['6', '1', '2', '3']);
		expect(call<Playlist>('GET', `/playlists/${made.id}`).body).toEqual(made);
	});

	it('gives an empty playlist no image, and a full one a mosaic', () => {
		expect(create().image).toBeNull();
		expect(create({ title: 'Full', items: [{ type: 'track', id: '1' }] }).image).not.toBeNull();
	});

	it('refuses what is not a playlist, pointing at the field', () => {
		const refused = (body: object) => call<Problem>('POST', '/playlists', body).body;
		expect(refused({})).toMatchObject({ status: 422, code: 'validation_failed' });
		expect(refused({ title: ' ' }).detail).toBe('A playlist needs a title. (at /title)');
		const twoTracks = {
			title: 'x',
			items: [
				{ type: 'track', id: '1' },
				{ type: 'track', id: '9' }
			]
		};
		expect(field(refused(twoTracks))).toBe('/items/1');
		expect(field(refused({ title: 'x', fromMix: 'm' }))).toBe('/fromMix');
	});

	it('refuses tracks from another library', () => {
		const out = call<Problem>('POST', '/playlists', {
			title: 'x',
			items: [{ type: 'track', id: '7' }]
		});
		expect(out.status).toBe(422);
	});

	it('creates one playlist however often a key is retried', () => {
		const first = create({ title: 'Once' }, 'key-1');
		const again = create({ title: 'Once' }, 'key-1');
		expect(again).toEqual(first);
		expect(store.all(LIB).filter((p) => p.title === 'Once')).toHaveLength(1);
	});
});

describe('editing', () => {
	const edit = (body: object, key?: string, path = '/playlists/pl-seed') =>
		call<Playlist & Problem>('PATCH', path, body, key);

	it('applies details and ops as one edit, and advances the version', () => {
		const out = edit({
			baseVersion: 1,
			title: 'Renamed',
			ops: [
				{ op: 'add', items: [{ type: 'artist', id: '200' }] },
				{ op: 'move', positions: [0, 3], at: 2 }
			]
		});
		expect(out.status).toBe(200);
		expect(out.body).toMatchObject({ title: 'Renamed', version: 2, trackCount: 4 });
		expect(out.body.durationUs).toBe(12_000_000);
		expect(ids('/playlists/pl-seed')).toEqual(['2', '4', '1', '5']);
		expect(call<PlaylistItemPage>('GET', '/playlists/pl-seed/items').body.version).toBe(2);
	});

	it('takes edits chained on the versions they produce', () => {
		expect(edit({ baseVersion: 1, ops: [{ op: 'remove', positions: [0] }] }).status).toBe(200);
		expect(edit({ baseVersion: 2, ops: [{ op: 'remove', positions: [0] }] }).status).toBe(200);
		expect(store.get(LIB, 'pl-seed')).toMatchObject({ version: 3, trackCount: 0, image: null });
	});

	it('refuses an edit made against an older version, applying none of it', () => {
		edit({ baseVersion: 1, pinned: true });
		const out = edit({ baseVersion: 1, title: 'Stale', ops: [{ op: 'remove', positions: [0] }] });
		expect(out.status).toBe(409);
		expect(out.body).toMatchObject({ code: 'version_conflict', currentVersion: 2 });
		expect(store.get(LIB, 'pl-seed')).toMatchObject({ title: 'Seed', version: 2, trackCount: 2 });
	});

	it('answers a retried edit as it first did, rather than as a conflict with itself', () => {
		const body = { baseVersion: 1, ops: [{ op: 'add', items: [{ type: 'track', id: '6' }] }] };
		const first = edit(body, 'key-2');
		const retry = edit(body, 'key-2');
		expect(retry).toEqual(first);
		expect(ids('/playlists/pl-seed')).toEqual(['1', '2', '6']);
	});

	it('refuses a position out of range, applying none of the edit', () => {
		const out = edit({
			baseVersion: 1,
			title: 'Half',
			ops: [
				{ op: 'remove', positions: [0] },
				{ op: 'remove', positions: [1] }
			]
		});
		expect(out.status).toBe(422);
		expect(field(out.body)).toBe('/ops');
		expect(store.get(LIB, 'pl-seed')).toMatchObject({ title: 'Seed', version: 1, trackCount: 2 });
	});

	it('refuses malformed edits', () => {
		const at = (body: object) => field(edit(body).body);
		expect(at({ title: 'No version' })).toBe('/baseVersion');
		expect(at({ baseVersion: 1 })).toBe('');
		expect(at({ baseVersion: 1, title: '' })).toBe('/title');
		expect(at({ baseVersion: 1, pinned: 'yes' })).toBe('/pinned');
		expect(at({ baseVersion: 1, ops: [] })).toBe('/ops');
		expect(at({ baseVersion: 1, ops: [{ op: 'shuffle' }] })).toBe('/ops/0/op');
		expect(at({ baseVersion: 1, ops: [{ op: 'remove', positions: 0 }] })).toBe('/ops/0/positions');
		expect(at({ baseVersion: 1, ops: [{ op: 'add', items: [{ type: 'mix', id: '1' }] }] })).toBe(
			'/ops/0/items/0'
		);
	});

	it('adds a playlist as its tracks', () => {
		const made = create({ title: 'Mine' });
		edit(
			{ baseVersion: 1, ops: [{ op: 'add', items: [{ type: 'playlist', id: 'pl-seed' }] }] },
			undefined,
			`/playlists/${made.id}`
		);
		expect(ids(`/playlists/${made.id}`)).toEqual(['1', '2']);
	});

	it('gives the mosaic a new ID when the contents change', () => {
		const before = store.get(LIB, 'pl-seed')!.image!.id;
		const after = edit({ baseVersion: 1, ops: [{ op: 'move', positions: [0], at: 1 }] }).body.image!
			.id;
		expect(after).not.toBe(before);
		expect(store.trackIds(LIB, 'pl-seed')).toEqual([2n, 1n]);
	});

	it('answers 404 for a playlist that does not exist', () => {
		expect(edit({ baseVersion: 1, pinned: true }, undefined, '/playlists/nope').status).toBe(404);
	});
});

describe('deleting', () => {
	it('deletes once', () => {
		expect(call('DELETE', '/playlists/pl-seed').status).toBe(204);
		expect(call('GET', '/playlists/pl-seed').status).toBe(404);
		expect(call('DELETE', '/playlists/pl-seed').status).toBe(404);
	});
});

describe('duplicates', () => {
	it('names the tracks already in the playlist, once each', () => {
		const out = call<{ trackIds: string[] }>('POST', '/playlists/pl-seed/duplicates', {
			items: [
				{ type: 'track', id: '2' },
				{ type: 'album', id: '100' },
				{ type: 'track', id: '6' }
			]
		});
		expect(out.body).toEqual({ trackIds: ['2', '1'] });
	});
});

it('refuses methods a path does not take', () => {
	expect(call<Problem>('PUT', '/playlists/pl-seed').body.code).toBe('method_not_allowed');
});

describe('Liked Songs', () => {
	const liked = () => ids('/playlists/liked-1');

	it('takes a like at the top, once, and shows it on the track', () => {
		expect([1n, 2n, 1n].map((t) => store.like(LIB, t))).toEqual([true, true, true]);
		expect(liked()).toEqual(['2', '1']);
		expect(store.get(LIB, 'liked-1')).toMatchObject({ trackCount: 2, version: 3, image: null });
		const tracks = [track(1), track(3)];
		store.markLiked(LIB, tracks);
		expect(tracks.map((t) => t.liked)).toEqual([true, false]);
		store.unlike(LIB, 1n);
		store.unlike(LIB, 1n);
		expect(liked()).toEqual(['2']);
		expect(store.like(LIB, 7n)).toBe(false);
	});

	it('keeps its name, and an add to it likes', () => {
		store.like(LIB, 1n);
		const renamed = call<Problem>('PATCH', '/playlists/liked-1', {
			baseVersion: 2,
			title: 'Faves'
		});
		expect(renamed.status).toBe(422);
		const items = [2, 1, 2].map((id) => ({ type: 'track', id: String(id) }));
		const added = call('PATCH', '/playlists/liked-1', {
			baseVersion: 2,
			ops: [{ op: 'add', items }]
		});
		expect(added.status).toBe(200);
		expect(liked()).toEqual(['2', '1']);
	});

	it('is made again, pinned, by a like after a delete', () => {
		expect(call('DELETE', '/playlists/liked-1').status).toBe(204);
		store.like(LIB, 3n);
		const again = store.all(LIB).find((p) => p.kind === 'liked');
		expect(again).toMatchObject({ pinned: true, trackCount: 1 });
	});
});

describe('hidden tracks', () => {
	const hidden = (order = '') =>
		store.hiddenTracks(LIB, new URLSearchParams(order)).items.map((t) => t.id);

	it('lists them newest first, and each once', () => {
		expect([1n, 2n, 1n].map((t) => store.hide(LIB, t))).toEqual([true, true, true]);
		expect(hidden()).toEqual(['2', '1']);
		expect(hidden('order=asc')).toEqual(['1', '2']);
		store.unhide(LIB, 2n);
		expect(hidden()).toEqual(['1']);
		expect(store.hide(LIB, 7n)).toBe(false);
	});

	it('and likes undo each other', () => {
		store.like(LIB, 1n);
		store.hide(LIB, 1n);
		expect(ids('/playlists/liked-1')).toEqual([]);
		store.like(LIB, 1n);
		expect(hidden()).toEqual([]);
		store.hide(LIB, 2n);
		const items = [{ type: 'track', id: '2' }];
		call('PATCH', '/playlists/liked-1', { baseVersion: 4, ops: [{ op: 'add', items }] });
		expect(hidden()).toEqual([]);
	});
});

describe('a hidden track in a playlist', () => {
	it('keeps its place, marked', () => {
		store.hide(LIB, 1n);
		const items = call<PlaylistItemPage>('GET', '/playlists/pl-seed/items').body.items;
		expect(items.map((i) => [i.track?.id, i.hidden])).toEqual([
			['1', true],
			['2', false]
		]);
	});
});
