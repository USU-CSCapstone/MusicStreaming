import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Playlist, PlaylistItem, TrackSummary } from './api/types';
import { CONFLICT_MESSAGE, EditablePlaylist, loadPlaylist } from './playlist.svelte';
import { toaster } from './toast.svelte';

const track = (n: number): TrackSummary => ({
	id: String(n),
	title: `Track ${n}`,
	artists: [],
	album: { id: '100', title: 'Album', artists: [], image: null },
	durationUs: n * 1_000_000,
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available'
});
const item = (n: number): PlaylistItem => ({
	track: track(n),
	addedAt: '2026-01-01T00:00:00.000Z'
});
const ids = (items: PlaylistItem[]) => items.map((i) => i.track?.id);

const playlist: Playlist = {
	id: 'p1',
	libraryId: '11',
	title: 'Road Trip',
	description: null,
	image: { id: 'mosaic-1', placeholder: '' },
	customImage: false,
	trackCount: 3,
	durationUs: 6_000_000,
	pinned: false,
	createdAt: '2026-01-01T00:00:00.000Z',
	modifiedAt: '2026-01-01T00:00:00.000Z',
	version: 1
};

type Request = { method: string; path: string; body: Record<string, unknown>; key: string | null };
type Reply = { status: number; body?: unknown };

/**
 * A server holding the playlist at a version. PATCH succeeds against the current version and
 * advances it, and is refused with `version_conflict` otherwise. `next` answers the following
 * request instead, once, and may hold it open to see what is sent meanwhile.
 */
function server() {
	const state = {
		version: 1,
		items: [item(1), item(2), item(3)],
		details: {} as Record<string, unknown>
	};
	const requests: Request[] = [];
	const next: ((r: Request) => Reply | Promise<Reply>)[] = [];

	function answer(r: Request): Reply {
		if (r.method === 'GET' && r.path.endsWith('/items')) {
			return {
				status: 200,
				body: { items: state.items, nextCursor: null, total: 3, version: state.version }
			};
		}
		if (r.method === 'GET') {
			return { status: 200, body: { ...playlist, ...state.details, version: state.version } };
		}
		if (r.body.baseVersion !== state.version) {
			return {
				status: 409,
				body: { status: 409, code: 'version_conflict', currentVersion: state.version }
			};
		}
		state.version++;
		for (const k of ['title', 'description', 'pinned']) {
			if (k in r.body) state.details[k] = r.body[k];
		}
		return {
			status: 200,
			body: {
				...playlist,
				...state.details,
				version: state.version,
				image: { id: `mosaic-${state.version}` }
			}
		};
	}

	const fetch = async (input: RequestInfo | URL, init: RequestInit = {}) => {
		const url = new URL(String(input), 'http://test');
		const request: Request = {
			method: init.method ?? 'GET',
			path: url.pathname.replace(/^.*\/api\/v1/, ''),
			body: init.body ? JSON.parse(String(init.body)) : {},
			key: new Headers(init.headers).get('Idempotency-Key')
		};
		requests.push(request);
		const reply = await (next.shift() ?? answer)(request);
		return new Response(JSON.stringify(reply.body), { status: reply.status });
	};

	const edits = () => requests.filter((r) => r.method === 'PATCH');
	return { state, requests, edits, next, answer, fetch: fetch as typeof globalThis.fetch };
}

let api: ReturnType<typeof server>;
let list: EditablePlaylist;

beforeEach(() => {
	api = server();
	list = new EditablePlaylist(api.fetch, { playlist, items: [item(1), item(2), item(3)] });
});

afterEach(() => {
	toaster.dismiss();
	vi.useRealTimers();
});

describe('editing', () => {
	it('shows an edit at once, totals included, before the server answers', () => {
		list.edit({ title: 'Renamed', ops: [{ op: 'remove', positions: [0] }] });
		expect(ids(list.items)).toEqual(['2', '3']);
		expect(list.playlist).toMatchObject({ title: 'Renamed', trackCount: 2, durationUs: 5_000_000 });
		expect(list.saving).toBe(true);
	});

	it('shows added tracks from their summaries', () => {
		list.edit({ ops: [{ op: 'add', items: [{ type: 'track', id: '9' }], at: 0 }] }, [track(9)]);
		expect(ids(list.items)).toEqual(['9', '1', '2', '3']);
		expect(list.playlist.durationUs).toBe(15_000_000);
	});

	it('refuses an add it has no summary for, and a position out of range, applying nothing', () => {
		expect(() =>
			list.edit({ ops: [{ op: 'add', items: [{ type: 'album', id: '100' }] }] })
		).toThrow();
		expect(() => list.edit({ ops: [{ op: 'remove', positions: [3] }] })).toThrow(RangeError);
		expect(ids(list.items)).toEqual(['1', '2', '3']);
		expect(list.saving).toBe(false);
		expect(api.requests).toEqual([]);
	});

	it('takes the server’s playlist once nothing is pending', async () => {
		list.edit({ ops: [{ op: 'move', positions: [0], at: 2 }] });
		await list.idle();
		expect(list.saving).toBe(false);
		expect(list.playlist).toMatchObject({ version: 2, image: { id: 'mosaic-2' } });
		expect(ids(list.items)).toEqual(['2', '3', '1']);
	});

	it('clears the artwork at once when the playlist empties', () => {
		list.edit({ ops: [{ op: 'remove', positions: [0, 1, 2] }] });
		expect(list.playlist.image).toBeNull();
	});
});

describe('sending', () => {
	it('sends one edit at a time, each against the version the last produced', async () => {
		let release!: () => void;
		api.next.push((r) => new Promise((done) => (release = () => done(api.answer(r)))));
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ pinned: true });
		await vi.waitFor(() => expect(api.edits()).toHaveLength(1));
		release();
		await list.idle();
		expect(api.edits().map((r) => r.body.baseVersion)).toEqual([1, 2, 3]);
		expect(api.edits()[0].body.ops).toEqual([{ op: 'remove', positions: [0] }]);
		expect(list.playlist).toMatchObject({ version: 4 });
		expect(ids(list.items)).toEqual(['3']);
		expect(toaster.current).toBeNull();
	});

	it('keeps pending details over the server’s answers to earlier edits', async () => {
		let release!: () => void;
		api.next.push((r) => new Promise((done) => (release = () => done(api.answer(r)))));
		list.edit({ pinned: true });
		list.edit({ title: 'Later' });
		await vi.waitFor(() => expect(api.edits()).toHaveLength(1));
		release();
		await vi.waitFor(() => expect(api.edits()).toHaveLength(2));
		// The first is confirmed and the second still pending: both show.
		expect(list.playlist).toMatchObject({ pinned: true, title: 'Later' });
		await list.idle();
	});

	it('retries an unanswered edit with the same key until it gets through', async () => {
		vi.useFakeTimers();
		api.next.push(() => Promise.reject(new TypeError('Failed to fetch')));
		api.next.push(() => ({ status: 503, body: { status: 503, code: 'internal' } }));
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		await vi.advanceTimersByTimeAsync(1000);
		await vi.advanceTimersByTimeAsync(2000);
		await list.idle();
		const edits = api.edits();
		expect(edits).toHaveLength(3);
		expect(new Set(edits.map((r) => r.key)).size).toBe(1);
		expect(edits[0].key).toBeTruthy();
		expect(edits.every((r) => r.body.baseVersion === 1)).toBe(true);
		expect(ids(list.items)).toEqual(['2', '3']);
		expect(list.playlist.version).toBe(2);
	});

	it('gives each edit its own key', async () => {
		list.edit({ pinned: true });
		list.edit({ pinned: false });
		await list.idle();
		const [a, b] = api.edits();
		expect(a.key).not.toBe(b.key);
	});
});

describe('undo', () => {
	it('puts removed tracks back where they were, and saves that too', async () => {
		const undo = list.edit({ ops: [{ op: 'remove', positions: [0, 2] }] });
		expect(ids(list.items)).toEqual(['2']);
		await list.idle();
		undo();
		expect(ids(list.items)).toEqual(['1', '2', '3']);
		expect(list.playlist.trackCount).toBe(3);
		await list.idle();
		expect(api.edits().at(-1)?.body.ops).toEqual([
			{ op: 'add', at: 0, items: [{ type: 'track', id: '1' }] },
			{ op: 'add', at: 2, items: [{ type: 'track', id: '3' }] }
		]);
		expect(list.playlist.version).toBe(3);
	});

	it('reverses details too', () => {
		const undo = list.edit({ title: 'Renamed', pinned: true });
		undo();
		expect(list.playlist).toMatchObject({ title: 'Road Trip', pinned: false });
	});

	it('does nothing once the contents have changed again', () => {
		const undo = list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ ops: [{ op: 'move', positions: [0], at: 1 }] });
		undo();
		expect(ids(list.items)).toEqual(['3', '2']);
	});

	it('still undoes after a change to the details alone', () => {
		const undo = list.edit({ ops: [{ op: 'remove', positions: [1] }] });
		list.edit({ pinned: true });
		undo();
		expect(ids(list.items)).toEqual(['1', '2', '3']);
		expect(list.playlist.pinned).toBe(true);
	});
});

describe('when the server refuses', () => {
	it('reloads after a conflict, drops what was pending, and says why', async () => {
		// Another device moved the playlist on, and left one track.
		api.state.version = 5;
		api.state.items = [item(3)];
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ title: 'Mine' });
		await list.idle();
		expect(api.edits()).toHaveLength(1);
		expect(ids(list.items)).toEqual(['3']);
		expect(list.playlist).toMatchObject({ version: 5, title: 'Road Trip' });
		expect(list.saving).toBe(false);
		expect(toaster.current?.message).toBe(CONFLICT_MESSAGE);
	});

	it('edits the reloaded playlist afterwards', async () => {
		api.state.version = 5;
		list.edit({ pinned: true });
		await list.idle();
		list.edit({ pinned: true });
		await list.idle();
		expect(api.edits().at(-1)?.body.baseVersion).toBe(5);
		expect(list.playlist.version).toBe(6);
	});

	it('rolls back everything pending on any other refusal, and shows it', async () => {
		api.next.push(() => ({
			status: 422,
			body: { status: 422, code: 'validation_failed', detail: 'Position 7 is out of range.' }
		}));
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ title: 'Built on the first' });
		await list.idle();
		expect(api.edits()).toHaveLength(1);
		expect(ids(list.items)).toEqual(['1', '2', '3']);
		expect(list.playlist).toEqual(playlist);
		expect(toaster.current?.message).toBe('Couldn’t save your change. Position 7 is out of range.');
	});
});

describe('loadPlaylist', () => {
	it('loads the playlist and its items at one version', async () => {
		const loaded = await loadPlaylist(api.fetch, '11', 'p1');
		expect(loaded.playlist.version).toBe(1);
		expect(ids(loaded.items)).toEqual(['1', '2', '3']);
	});

	it('tries again when the playlist changes while it loads', async () => {
		api.next.push((r) => {
			const reply = api.answer(r);
			api.state.version = 2; // Changed after the items were read.
			return reply;
		});
		const loaded = await loadPlaylist(api.fetch, '11', 'p1');
		expect(loaded.playlist.version).toBe(2);
		expect(api.requests.filter((r) => r.path.endsWith('/items'))).toHaveLength(2);
	});
});
