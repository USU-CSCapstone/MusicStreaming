import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Playlist, PlaylistItem, TrackSummary } from './api/types';
import { CONFLICT_MESSAGE, EditablePlaylist, loadPlaylist, rowMoveOps } from './playlist.svelte';
import { connectivity } from './connectivity.svelte';
import type { Device } from './sync/device';
import type { Local } from './sync/mutation';
import { sender } from './sync/sender';
import { type Env, memoryEnv } from './sync/store';
import { applyOps } from './playlist-ops';
import { toaster } from './toast.svelte';

const track = (n: number): TrackSummary => ({
	id: String(n),
	title: `Track ${n}`,
	artists: [],
	album: { id: '100', title: 'Album', artists: [], image: null },
	discNumber: null,
	trackNumber: n,
	durationUs: n * 1_000_000,
	releaseDate: null,
	tags: {},
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available',
	sortKeys: { title: `track ${n}`, artist: '', album: 'album' },
	liked: false
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
	image: { id: 'mosaic-1', placeholder: null },
	kind: 'saved',
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

/** Whether the fake server stores the tracks an add names, so what it holds can be read back. */
const applies = { on: false };

/**
 * A server holding the playlist at a version. PATCH succeeds against the current version and
 * advances it, and is refused with `version_conflict` otherwise. `next` answers the following
 * request instead, once, and may hold it open to see what is sent meanwhile. With `editing`, the
 * first plain GET (the version read before a first edit) is answered but left out of `requests`.
 */
function server(editing = true) {
	const state = {
		version: 1,
		items: [item(1), item(2), item(3)],
		details: {} as Record<string, unknown>
	};
	const requests: Request[] = [];
	const next: ((r: Request) => Reply | Promise<Reply>)[] = [];
	// An edited playlist's first edit reads its version from the server; that read is not recorded.
	let unrecorded = editing;

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
		if (r.body.ops)
			for (const op of r.body.ops as {
				op: string;
				items?: { id: string }[];
				positions?: number[];
			}[]) {
				if (!applies.on) continue;
				if (op.op === 'add')
					state.items = [...state.items, ...op.items!.map((p) => item(Number(p.id)))];
				if (op.op === 'remove')
					state.items = state.items.filter((_, i) => !op.positions!.includes(i));
			}
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
		if (unrecorded && request.method === 'GET' && !request.path.endsWith('/items')) {
			unrecorded = false;
			return Response.json(answer(request).body);
		}
		requests.push(request);
		const reply = await (next.shift() ?? answer)(request);
		return new Response(JSON.stringify(reply.body), { status: reply.status });
	};

	const edits = () => requests.filter((r) => r.method === 'PATCH');
	return { state, requests, edits, next, answer, fetch: fetch as typeof globalThis.fetch };
}

let api: ReturnType<typeof server>;
let env: Env;
let list: EditablePlaylist;
/** What the device was asked to make. */
let made: Local[];
/** Settles the device's making of each change, which is at once unless held. */
let making: Promise<void>;
const device: Device = {
	apply: async (_user, local, answered) => {
		await making;
		// What it is asked to make, not what it is told was answered.
		if (!answered) made.push(local);
	},
	rebase: async () => {},
	confirmed: async () => {}
};

beforeEach(async () => {
	applies.on = false;
	api = server();
	env = memoryEnv();
	made = [];
	making = Promise.resolve();
	sender.start('u', { env, fetch: api.fetch, device });
	list = new EditablePlaylist(api.fetch, { playlist, items: [item(1), item(2), item(3)] });
});

afterEach(() => {
	sender.stop();
	connectivity.setWorkOffline(false);
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

	it('is saving until the server has the edit, at the version the edit made', async () => {
		list.edit({ ops: [{ op: 'move', positions: [0], at: 2 }] });
		expect(list.playlist.version).toBe(2);
		await list.idle();
		expect(list.saving).toBe(false);
		expect(api.edits().map((r) => r.body.baseVersion)).toEqual([1]);
		expect(ids(list.items)).toEqual(['2', '3', '1']);
	});

	it('has the device make the edit with its tracks, and says what it was made on', async () => {
		list.edit(
			{ title: 'Renamed', ops: [{ op: 'add', items: [{ type: 'track', id: '9' }], at: 0 }] },
			[track(9)]
		);
		await vi.waitFor(() => expect(made).toHaveLength(1));
		expect(made[0]).toMatchObject({
			library: '11',
			calls: [
				{
					name: 'editPlaylist',
					args: {
						playlistId: 'p1',
						baseVersion: 1,
						title: 'Renamed',
						ops: [{ op: 'add', items: ['9'], at: 0 }]
					}
				}
			]
		});
		await list.idle();
	});

	it('clears the artwork at once when the playlist empties', () => {
		list.edit({ ops: [{ op: 'remove', positions: [0, 1, 2] }] });
		expect(list.playlist.image).toBeNull();
	});
});

describe('opened again', () => {
	it('is saving while this tab holds changes to it unsent, and leaves showing them to the device', async () => {
		connectivity.setWorkOffline(true);
		list.edit({ ops: [{ op: 'add', items: [{ type: 'track', id: '9' }], at: 0 }] }, [track(9)]);
		await sender.idle();

		// What the device reads holds the edit; nothing applies it again here.
		const again = new EditablePlaylist(api.fetch, {
			playlist: { ...playlist, version: 2 },
			items: [item(9), item(1), item(2), item(3)]
		});
		expect(ids(again.items)).toEqual(['9', '1', '2', '3']);
		expect(again.saving).toBe(true);
		connectivity.setWorkOffline(false);
		sender.wake();
		await again.idle();
		expect(again.saving).toBe(false);
		expect(api.edits().map((r) => r.body.baseVersion)).toEqual([1]);
	});
});

describe('leaving the page and coming back', () => {
	/** An edit that adds track `n`. */
	const add = (n: number) => ({
		ops: [{ op: 'add' as const, items: [{ type: 'track' as const, id: String(n) }] }]
	});

	it('keeps both offline edits made on either side, as the route builds the page again', async () => {
		applies.on = true;
		connectivity.setWorkOffline(true);
		list.edit(add(9), [track(9)]);
		await sender.idle();
		// As the device reads it, with the first edit made.
		const again = new EditablePlaylist(api.fetch, {
			playlist: { ...playlist, version: 2 },
			items: [item(1), item(2), item(3), item(9)]
		});
		again.edit(add(8), [track(8)]);
		connectivity.setWorkOffline(false);
		sender.wake();
		await again.idle();
		expect(api.edits().map((r) => r.body.baseVersion)).toEqual([1, 2]);
		expect(api.state.items.map((i) => i.track?.id)).toEqual(['1', '2', '3', '9', '8']);
		expect(toaster.current).toBeNull();
	});

	it('keeps them too where the page was read before the first reached the device', async () => {
		applies.on = true;
		connectivity.setWorkOffline(true);
		list.edit(add(9), [track(9)]);
		await sender.idle();
		const again = new EditablePlaylist(api.fetch, { playlist, items: [item(1), item(2), item(3)] });
		again.edit(add(8), [track(8)]);
		connectivity.setWorkOffline(false);
		sender.wake();
		await sender.idle();
		expect(api.edits().map((r) => r.body.baseVersion)).toEqual([1, 2]);
		expect(api.state.items.map((i) => i.track?.id)).toEqual(['1', '2', '3', '9', '8']);
		expect(toaster.current).toBeNull();
	});
});

describe('refresh', () => {
	const newer = {
		playlist: { ...playlist, title: 'Elsewhere', version: 2 },
		items: [item(3), item(1)]
	};

	it('shows another device’s edit', () => {
		list.refresh(newer);
		expect(list.playlist).toMatchObject({ title: 'Elsewhere', version: 2 });
		expect(ids(list.items)).toEqual(['3', '1']);
	});

	it('ignores a playlist no newer than the one shown', () => {
		list.refresh({ playlist, items: [item(9)] });
		expect(ids(list.items)).toEqual(['1', '2', '3']);
	});

	it('ignores a read from before the device made an edit made here, and shows a newer one after', async () => {
		let release!: () => void;
		making = new Promise((resolve) => (release = resolve));
		list.edit({ title: 'Mine' });
		list.refresh({ ...newer, playlist: { ...newer.playlist, version: 3 } });
		expect(list.playlist.title).toBe('Mine');
		// The read held back is offered again once the device has the edit.
		release();
		await vi.waitFor(() => expect(list.playlist.title).toBe('Elsewhere'));
		list.refresh({ playlist: { ...playlist, title: 'Older', version: 2 }, items: [] });
		expect(list.playlist.title).toBe('Elsewhere');
		await list.idle();
	});

	it('shows what is read after a refusal, whatever its version', async () => {
		api.next.push(() => ({ status: 409, body: { status: 409, code: 'version_conflict' } }));
		api.state.items = [item(7)];
		list.edit({ title: 'Mine' });
		await list.idle();
		expect(toaster.current?.message).toBe(CONFLICT_MESSAGE);
		await vi.waitFor(() => expect(ids(list.items)).toEqual(['7']));
		expect(list.playlist.title).toBe('Road Trip');
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
		expect(list.playlist).toMatchObject({ version: 4, pinned: true });
		expect(ids(list.items)).toEqual(['3']);
		expect(toaster.current).toBeNull();
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

	it('says items were added here, by an undo too', async () => {
		expect(list.added).toBe(false);
		const undo = list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		expect(list.added).toBe(false);
		undo();
		expect(list.added).toBe(true);
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

describe('a Liked Songs', () => {
	const add = (...tracks: number[]): PlaylistItem[] => {
		const items = tracks.map((n) => ({ type: 'track' as const, id: String(n) }));
		liked.edit({ ops: [{ op: 'add', items }] }, tracks.map(track));
		return liked.items;
	};
	let liked: EditablePlaylist;
	beforeEach(() => {
		const kind = 'liked' as const;
		liked = new EditablePlaylist(api.fetch, {
			playlist: { ...playlist, kind, title: 'Liked Songs' },
			items: [item(1), item(2), item(3)]
		});
	});

	it('puts adds at the top, leaving out what it holds, as the server does', async () => {
		expect(ids(add(2, 9, 9, 4))).toEqual(['9', '4', '1', '2', '3']);
		await liked.idle();
		// The device is given the add as made, and makes it by the same rule.
		expect(made.at(-1)?.calls[0].args).toMatchObject({
			ops: [{ op: 'add', items: ['2', '9', '9', '4'] }]
		});
	});

	it('adds nothing for a song liked already, so later edits name the right positions', async () => {
		expect(ids(add(2))).toEqual(['1', '2', '3']);
		liked.edit({ ops: [{ op: 'remove', positions: [2] }] });
		await liked.idle();
		expect(api.edits().at(-1)?.body.ops).toEqual([{ op: 'remove', positions: [2] }]);
		expect(ids(liked.items)).toEqual(['1', '2']);
	});

	it('undoes an add by taking out only what it put in', () => {
		const undo = liked.edit(
			{
				ops: [
					{
						op: 'add',
						items: [
							{ type: 'track', id: '1' },
							{ type: 'track', id: '9' }
						]
					}
				]
			},
			[track(1), track(9)]
		);
		undo();
		expect(ids(liked.items)).toEqual(['1', '2', '3']);
	});
});

describe('when the server refuses', () => {
	it('shows the playlist read again after a conflict, drops what was pending, and says why', async () => {
		// Another device moved the playlist on, and left one track.
		api.state.version = 5;
		api.state.items = [item(3)];
		list.edit({ ops: [{ op: 'remove', positions: [0] }] });
		list.edit({ title: 'Mine' });
		await list.idle();
		// The first edit conflicts, and the server's items are not the ones it was made on.
		expect(api.edits()).toHaveLength(1);
		await vi.waitFor(() => expect(ids(list.items)).toEqual(['3']));
		expect(list.playlist).toMatchObject({ version: 5, title: 'Road Trip' });
		expect(list.saving).toBe(false);
		expect(toaster.current?.message).toBe(CONFLICT_MESSAGE);
	});

	it('edits the reloaded playlist afterwards', async () => {
		api.state.version = 5;
		api.state.items = [item(3)];
		list.edit({ pinned: true });
		await list.idle();
		await vi.waitFor(() => expect(list.playlist.version).toBe(5));
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
		await vi.waitFor(() => expect(ids(list.items)).toEqual(['1', '2', '3']));
		expect(list.playlist).toEqual(playlist);
		expect(toaster.current?.message).toBe('Couldn’t save your change. Position 7 is out of range.');
	});
});

describe('loadPlaylist', () => {
	beforeEach(() => {
		api = server(false);
	});

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

describe('rowMoveOps', () => {
	// Rows show tracks only, so the external item is in the playlist but not in the list.
	const external = { external: { id: 'e', durationUs: 0 } } as unknown as PlaylistItem;
	const items = [item(1), external, item(2), item(3), item(4)];
	const order = (moves: { indexes: number[]; to: number }[]) =>
		applyOps(items, rowMoveOps(items, moves), () => []).map((i) => i.track?.id ?? 'ext');

	it('moves rows around an external item the list does not show', () => {
		// The rows are 1, 2, 3, 4.
		expect(order([{ indexes: [0], to: 1 }])).toEqual(['ext', '2', '1', '3', '4']);
		expect(order([{ indexes: [3], to: 0 }])).toEqual(['4', '1', 'ext', '2', '3']);
	});

	it('moves rows around items it is told the list does not show, such as hidden ones', () => {
		// Track 2 is hidden, so the rows are 1, 3, 4.
		const shows = (i: PlaylistItem) => !!i.track && i.track.id !== '2';
		const moved = (moves: { indexes: number[]; to: number }[]) =>
			applyOps(items, rowMoveOps(items, moves, shows), () => []).map((i) => i.track?.id ?? 'ext');
		expect(moved([{ indexes: [2], to: 0 }])).toEqual(['4', '1', 'ext', '2', '3']);
		expect(moved([{ indexes: [0], to: 1 }])).toEqual(['ext', '2', '3', '1', '4']);
	});

	it('gathers scattered rows, and moves them to the end', () => {
		expect(order([{ indexes: [0, 2], to: 2 }])).toEqual(['ext', '2', '4', '1', '3']);
	});

	it('makes each move on the rows as the one before left them', () => {
		// Rows 2 and 4 each up one place, as Alt+↑ does.
		expect(
			order([
				{ indexes: [1], to: 0 },
				{ indexes: [3], to: 2 }
			])
		).toEqual(['2', '1', 'ext', '4', '3']);
	});
});
