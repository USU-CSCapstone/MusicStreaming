import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayReport, Playlist } from '$lib/api/types';
import { connectivity } from '$lib/connectivity.svelte';
import { PlayRecorder } from '$lib/plays';
import { toaster } from '$lib/toast.svelte';
import type { Device } from './device';
import { fingerprint } from './fingerprint';
import type { Definition, Local, Mutation } from './mutation';
import { type Name, registry } from './mutations';
import type { MarkArgs } from './mutations/marks';
import { CONFLICT_MESSAGE, type EditArgs } from './mutations/playlists';
import { MAX_AGE_MS, MAX_TRIES, MAX_RETRY_MS, Sender } from './sender';
import { type Env, memoryEnv } from './store';

vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

const playlist = {
	id: 'p1',
	libraryId: '11',
	title: 'Road Trip',
	kind: 'saved',
	version: 1
} as unknown as Playlist;

/** A request the fake server received, with its query and idempotency key. */
type Request = {
	method: string;
	path: string;
	query: string;
	body: Record<string, unknown>;
	key: string | null;
};
/** What the fake server answers a request with; `retryAfter` is in seconds. */
type Reply = { status: number; body?: unknown; retryAfter?: number };

/**
 * A server that takes plays, likes, hides and playlist edits, and records them; `next` answers the
 * following requests instead.
 */
function server() {
	const state = { version: 1, items: [] as unknown[] };
	const requests: Request[] = [];
	const next: ((r: Request) => Reply | Promise<Reply>)[] = [];
	/** Answers every request `next` does not. */
	const rules = { answer: (r: Request): Reply => fine(r) };

	/**
	 * The server behaving: playlists at `state.version` holding `state.items`, edits against it
	 * applied, anything else a 204.
	 */
	function fine(r: Request): Reply {
		if (r.method === 'GET' && r.path.endsWith('/items'))
			return {
				status: 200,
				body: { items: state.items, nextCursor: null, total: 0, version: state.version }
			};
		if (r.method === 'GET') return { status: 200, body: { ...playlist, version: state.version } };
		if (r.method === 'PATCH') {
			if (r.body.baseVersion !== state.version)
				return { status: 409, body: { status: 409, code: 'version_conflict' } };
			return { status: 200, body: { ...playlist, version: ++state.version } };
		}
		return { status: 204 };
	}

	/**
	 * A fetch that records each request and answers with the next queued reply, else `rules.answer`.
	 */
	const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
		const url = new URL(String(input), 'http://test');
		const request: Request = {
			method: init.method ?? 'GET',
			path: url.pathname.replace(/^.*\/api\/v1/, ''),
			query: url.search,
			body: init.body ? JSON.parse(String(init.body)) : {},
			key: new Headers(init.headers).get('Idempotency-Key')
		};
		requests.push(request);
		const reply = await (next.shift() ?? rules.answer)(request);
		const headers = reply.retryAfter ? { 'Retry-After': String(reply.retryAfter) } : undefined;
		return reply.status === 204
			? new Response(null, { status: 204 })
			: new Response(JSON.stringify(reply.body ?? { status: reply.status, code: 'internal' }), {
					status: reply.status,
					headers
				});
	}) as typeof globalThis.fetch;

	/** The requests of `method` whose path ends with `suffix`. */
	const sent = (method: string, suffix = '') =>
		requests.filter((r) => r.method === method && r.path.endsWith(suffix));
	/** The play IDs sent, in the order the requests carried them. */
	const played = () =>
		sent('POST', '/me/plays').flatMap((r) =>
			(r.body.items as PlayReport[]).map((report) => report.playId)
		);
	/** Each request as `METHOD path`. */
	const lines = () => requests.map((r) => `${r.method} ${r.path}${r.query}`);
	return { state, requests, next, rules, fetch, sent, played, lines };
}

/** A play report with this ID and listen time. */
const report = (playId: string, listenTimeMs = 0): PlayReport =>
	({
		playId,
		libraryId: '11',
		trackId: '5',
		startedAt: '2026-01-01T00:00:00.000Z',
		listenTimeMs,
		end: null,
		context: { type: 'library' },
		origin: 'context'
	}) as PlayReport;

/** A like, unlike, hide or unhide of track `track` of library 1. */
const mark = (track: string, state: Partial<MarkArgs>): MarkArgs => ({
	library: '1',
	track,
	what: 'change it',
	...state
});

/** An edit of playlist `p1` made at `baseVersion` on an empty playlist. */
const edit = (baseVersion: number, change: EditArgs['change']): EditArgs => ({
	libraryId: '11',
	playlistId: 'p1',
	baseVersion,
	change,
	fingerprint: fingerprint([])
});

/** An append of track 9 to `p1`. */
const append = (tracks?: string[]) => ({
	libraryId: '11',
	playlistId: 'p1',
	items: [{ type: 'track' as const, id: '9' }],
	tracks,
	what: 'add to “Road Trip”'
});

/** A device that records what it is asked. */
function fakeDevice() {
	const applied: Local[] = [];
	const rebased: string[] = [];
	const confirmed: string[] = [];
	const answered: Local[] = [];
	const device: Device = {
		apply: async (_user, local, told) =>
			void (told ? answered : applied).push(structuredClone(local)),
		rebase: async (_user, library) => void rebased.push(library),
		confirmed: async (_user, library, playlist, version) =>
			void confirmed.push(`${library}/${playlist}@${version}`)
	};
	return { applied, answered, rebased, confirmed, device };
}

let api: ReturnType<typeof server>;
let env: Env;
let device: ReturnType<typeof fakeDevice>;
const opened: Sender[] = [];

/** A sender for user `user` on the shared database, as a tab opening the app. */
function open(user = 'u') {
	const sender = new Sender();
	opened.push(sender);
	sender.start(user, { env, fetch: api.fetch, device: device.device });
	return sender;
}

/** Everything the dead-letter store holds. */
const dead = () => env.open('u').all<Mutation>('dead');

beforeEach(() => {
	api = server();
	env = memoryEnv();
	device = fakeDevice();
	connectivity.setWorkOffline(false);
});

afterEach(() => {
	for (const sender of opened.splice(0)) sender.stop();
	connectivity.setWorkOffline(false);
	connectivity.reach();
	toaster.dismiss();
	vi.useRealTimers();
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

describe('what was made offline', () => {
	/** Makes mutations with `make` while working offline, closes the app, and opens it online. */
	async function reopenedAfter(make: (sender: Sender) => void) {
		connectivity.setWorkOffline(true);
		const before = open();
		make(before);
		await before.idle();
		expect(api.requests, 'nothing is sent while working offline').toEqual([]);
		before.stop();
		connectivity.setWorkOffline(false);
		const after = open();
		await after.idle();
		return after;
	}

	it('sends a play after the app reopens online, once', async () => {
		const after = await reopenedAfter((o) => {
			o.enqueue('play', report('p1'));
			o.enqueue('play', report('p2'));
		});
		expect(api.played()).toEqual(['p1', 'p2']);
		expect(await after.pending()).toBe(0);
	});

	it('sends the latest like or hide of a track, as one request', async () => {
		await reopenedAfter((o) => {
			o.enqueue('mark', mark('5', { liked: true, hidden: false }));
			o.enqueue('mark', mark('6', { liked: false }));
			o.enqueue('mark', mark('7', { hidden: true, liked: false }));
			o.enqueue('mark', mark('7', { hidden: false }));
		});
		expect(api.lines().sort()).toEqual([
			'DELETE /libraries/1/tracks/6/like',
			'DELETE /libraries/1/tracks/7/hidden',
			'DELETE /libraries/1/tracks/7/like',
			'PUT /libraries/1/tracks/5/like'
		]);
		// Each request under a key of its own.
		expect(new Set(api.requests.map((r) => r.key)).size).toBe(4);
	});

	it('sends a hide, and a like with the ID for Liked Songs the device made it under', async () => {
		await reopenedAfter((o) => {
			o.enqueue('mark', mark('5', { hidden: true, liked: false }));
			o.enqueue('mark', mark('6', { liked: true, hidden: false, likedId: '42' }));
		});
		expect(api.lines().sort()).toEqual([
			'PUT /libraries/1/tracks/5/hidden',
			'PUT /libraries/1/tracks/6/like?likedId=42'
		]);
	});

	it('applies three edits to one playlist, each against the version the last produced', async () => {
		const after = await reopenedAfter((o) => {
			o.enqueue('editPlaylist', edit(1, { title: 'One' }));
			o.enqueue('editPlaylist', edit(2, { pinned: true }));
			o.enqueue('editPlaylist', edit(3, { description: 'Three' }));
		});
		const edits = api.sent('PATCH');
		expect(edits.map((r) => r.body.baseVersion)).toEqual([1, 2, 3]);
		expect(new Set(edits.map((r) => r.key)).size).toBe(3);
		expect(edits.map((r) => Object.keys(r.body).sort().join())).toEqual([
			'baseVersion,title',
			'baseVersion,pinned',
			'baseVersion,description'
		]);
		expect(await after.pending()).toBe(0);
	});

	it('sends an edit on the version the one before produced, whatever it was made on', async () => {
		// Another device renamed it meanwhile, which the first edit is no conflict with.
		api.state.version = 4;
		await reopenedAfter((o) => {
			o.enqueue('editPlaylist', edit(1, { pinned: true }));
			o.enqueue('editPlaylist', edit(2, { title: 'Two' }));
		});
		expect(api.lines()).toEqual([
			'PATCH /libraries/11/playlists/p1',
			'GET /libraries/11/playlists/p1/items?limit=1000',
			'GET /libraries/11/playlists/p1',
			'PATCH /libraries/11/playlists/p1',
			'PATCH /libraries/11/playlists/p1'
		]);
		expect(api.sent('PATCH').map((r) => r.body.baseVersion)).toEqual([1, 4, 5]);
	});

	it('drops the edits of a playlist another device changed, takes them off the device and says so once', async () => {
		api.state.version = 4;
		api.state.items = [{ track: { id: '9' }, addedAt: '' }];
		const show = vi.spyOn(toaster, 'show');
		const after = await reopenedAfter((o) => {
			o.enqueue('editPlaylist', edit(1, { title: 'One' }));
			o.enqueue('editPlaylist', edit(2, { title: 'Two' }));
			// Made on no position, so it goes all the same.
			o.enqueue('appendToPlaylist', append());
		});
		expect(api.sent('PATCH').map((r) => r.body.baseVersion)).toEqual([1, 4]);
		expect(api.sent('PATCH')[1].body.ops).toEqual([
			{ op: 'add', items: [{ type: 'track', id: '9' }] }
		]);
		expect(await after.pending()).toBe(0);
		expect(show.mock.calls.map(([message]) => message)).toEqual([CONFLICT_MESSAGE]);
		expect(device.rebased).toEqual(['11']);
	});
});

describe('order', () => {
	it('sends a scope one at a time in order, and the scopes side by side', async () => {
		const held: (() => void)[] = [];
		api.rules.answer = (r) =>
			r.path.includes('/tracks/5')
				? (new Promise<Reply>((resolve) => held.push(() => resolve({ status: 204 }))) as never)
				: { status: 204 };
		const sender = open();
		sender.enqueue('mark', mark('5', { liked: true }));
		await vi.waitFor(() => expect(held).toHaveLength(1));
		// Folded into the like being sent, so it goes once the like is answered.
		sender.enqueue('mark', mark('5', { liked: false }));
		sender.enqueue('mark', mark('6', { hidden: true }));
		sender.enqueue('play', report('p1'));
		await vi.waitFor(() => expect(api.played()).toEqual(['p1']));
		expect(api.lines()).toEqual([
			'PUT /libraries/1/tracks/5/like',
			'PUT /libraries/1/tracks/6/hidden',
			'POST /me/plays'
		]);
		held.shift()!();
		await vi.waitFor(() => expect(held).toHaveLength(1));
		expect(api.lines().at(-1)).toBe('DELETE /libraries/1/tracks/5/like');
		held.shift()!();
		await sender.idle();
		expect(await sender.pending()).toBe(0);
	});

	it('folds a change into the unsent one of its scope, under a new key', async () => {
		connectivity.setWorkOffline(true);
		const sender = open();
		const first = sender.enqueue('mark', mark('5', { liked: true, hidden: false }))!;
		const second = sender.enqueue('mark', mark('5', { liked: false }))!;
		expect(second.key).toBe(first.key);
		await sender.idle();
		const [kept] = await env.open('u').all<Mutation<MarkArgs>>('mutations');
		expect(kept.args).toMatchObject({ liked: false, hidden: false });
		// Both changes are on the device, in the order they were made.
		await Promise.all([first.applied, second.applied]);
		expect(device.applied.at(-1)!.calls.map((call) => call.name)).toEqual(['like', 'unlike']);
		connectivity.setWorkOffline(false);
		await sender.idle();
		expect(api.lines()).toEqual([
			'DELETE /libraries/1/tracks/5/like',
			'DELETE /libraries/1/tracks/5/hidden'
		]);
	});

	it('sends the plays of a day offline a batch at a time, the latest report of each', async () => {
		connectivity.setWorkOffline(true);
		const sender = open();
		for (let i = 0; i < 600; i++) sender.enqueue('play', report(`p${i}`, 1000));
		sender.enqueue('play', report('p0', 5000));
		connectivity.setWorkOffline(false);
		sender.wake();
		await sender.idle();
		const batches = api.sent('POST', '/me/plays');
		expect(batches.map((r) => (r.body.items as unknown[]).length)).toEqual([500, 100]);
		const first = (batches[0].body.items as PlayReport[]).find((p) => p.playId === 'p0');
		expect(first?.listenTimeMs).toBe(5000);
	});

	it('sends a kind sent many at a time with its own request, as many as it says', async () => {
		const sent: unknown[][] = [];
		const kinds = registry as unknown as Record<string, Definition<unknown>>;
		kinds.stamp = {
			scope: () => 'stamps',
			send: async () => {},
			batch: { size: 2, send: async (args) => void sent.push(args) },
			what: () => 'stamp it'
		};
		try {
			const sender = open();
			connectivity.setWorkOffline(true);
			for (const n of [1, 2, 3]) sender.enqueue('stamp' as Name, n as never);
			connectivity.setWorkOffline(false);
			sender.wake();
			await sender.idle();
			expect(sent).toEqual([[1, 2], [3]]);
			expect(api.requests).toEqual([]);
		} finally {
			delete kinds.stamp;
		}
	});
});

describe('the device', () => {
	it('is asked to make each change once it is in the log, with when it was made', async () => {
		const sender = open();
		const at = Date.now();
		const queued = sender.enqueue('mark', mark('5', { liked: true, hidden: false, likedId: '7' }))!;
		await queued.applied;
		const [kept] = await env.open('u').all<Mutation>('mutations');
		expect(kept?.key ?? queued.key).toBe(queued.key);
		expect(device.applied).toEqual([
			{
				library: '1',
				calls: [
					{
						id: `${queued.key}/0`,
						name: 'like',
						args: { trackId: '5', likedId: '7' },
						at: expect.any(Number)
					}
				]
			}
		]);
		expect(device.applied[0].calls[0].at).toBeGreaterThanOrEqual(at);
	});

	it('is asked to make nothing for a play, nor for an append whose songs it does not know', async () => {
		const sender = open();
		await sender.enqueue('play', report('p1'))!.applied;
		await sender.enqueue('appendToPlaylist', append())!.applied;
		await sender.enqueue('appendToPlaylist', append(['9']))!.applied;
		expect(device.applied.map((local) => local.calls.map((call) => call.name))).toEqual([
			['appendToPlaylist']
		]);
	});

	it('is told the version an append it made was answered with, once the append has left the log', async () => {
		// A database slow to delete, so a call made before the deletion is written would come first.
		const order: string[] = [];
		const quick = env;
		env = {
			...quick,
			open(user) {
				const port = quick.open(user);
				return {
					...port,
					remove: async (store, key, rev) => {
						await new Promise((done) => setTimeout(done, 20));
						await port.remove(store, key, rev);
						order.push(`removed ${key}`);
					}
				};
			}
		};
		const sender = open();
		device.device.confirmed = async (_user, library, playlist, version) => {
			order.push('confirmed');
			device.confirmed.push(`${library}/${playlist}@${version}`);
		};
		const made = sender.enqueue('appendToPlaylist', append(['9']))!;
		// Neither an append the device did not make nor an edit, which a stale version refuses.
		sender.enqueue('appendToPlaylist', append());
		sender.enqueue('editPlaylist', edit(3, { title: 'Later' }));
		await sender.idle();
		await vi.waitFor(() => expect(device.confirmed).toEqual(['11/p1@2']));
		expect(order.slice(0, 2)).toEqual([`removed ${made.key}`, 'confirmed']);
	});

	it('is told a change was answered, so it keeps it until the feed brings it', async () => {
		const sender = open();
		const queued = sender.enqueue('mark', mark('5', { hidden: true }))!;
		expect(await sender.settled(queued.key)).toBe('sent');
		expect(device.answered).toEqual(device.applied);
		expect(device.answered).toHaveLength(1);
		// Nothing for one the server refuses.
		api.next.push(() => ({ status: 404, body: { status: 404, code: 'not_found' } }));
		const refused = sender.enqueue('mark', mark('6', { hidden: true }))!;
		expect(await sender.settled(refused.key)).toBe('dropped');
		expect(device.answered).toHaveLength(1);
	});

	it('is asked to take back a refused change and those made on it, and only after', async () => {
		const show = vi.spyOn(toaster, 'show');
		api.next.push(() => ({
			status: 404,
			body: { status: 404, code: 'not_found', detail: 'Gone.' }
		}));
		let told = false;
		device.device.rebase = async (_user, library) => {
			device.rebased.push(library);
			told = true;
		};
		const sender = open();
		const queued = sender.enqueue('mark', mark('5', { hidden: true, what: 'hide “Signal”' }))!;
		const outcome = await sender.settled(queued.key);
		expect(outcome).toBe('dropped');
		expect(told).toBe(true);
		expect(device.rebased).toEqual(['1']);
		expect(show.mock.calls.map(([message]) => message)).toEqual(['Couldn’t hide “Signal”. Gone.']);
		expect(await sender.pending()).toBe(0);
	});
});

describe('a play checkpointed', () => {
	it('keeps its listen time after the page was closed without warning', async () => {
		connectivity.setWorkOffline(true);
		const before = open();
		const recorder = new PlayRecorder((plays) => {
			for (const play of plays) before.enqueue('play', play);
		});
		recorder.start('11', '5', { type: 'library' }, 'context');
		for (let t = 0.5; t <= 20; t += 0.5) recorder.heard(t);
		recorder.checkpoint();
		await before.idle();
		// Nothing more is reported: the page is gone.
		before.stop();

		connectivity.setWorkOffline(false);
		const after = open();
		await after.idle();
		const [sent] = api.sent('POST', '/me/plays');
		expect((sent.body.items as PlayReport[]).map((p) => p.listenTimeMs)).toEqual([20_000]);
	});
});

describe('what the server answers', () => {
	it('keeps everything on a 401, and sends it once told the session may be good', async () => {
		api.next.push(() => ({ status: 401, body: { status: 401, code: 'other' } }));
		const sender = open();
		sender.enqueue('play', report('p1'));
		await sender.idle();
		expect(api.played()).toEqual(['p1']);
		expect(await sender.pending()).toBe(1);
		// Further reports do not try again, which a session that is over would only refuse.
		sender.enqueue('play', report('p2'));
		await sender.idle();
		expect(api.played()).toEqual(['p1']);
		sender.wake();
		await sender.idle();
		expect(api.played()).toEqual(['p1', 'p1', 'p2']);
		expect(await sender.pending()).toBe(0);
	});

	it('keeps everything on a redirect to the login page, and does not try again by itself', async () => {
		vi.useFakeTimers();
		vi.stubGlobal('location', { pathname: '/', search: '' });
		api.next.push(() => ({ status: 401, body: { status: 401, code: 'unauthenticated' } }));
		const sender = open();
		sender.enqueue('play', report('p1'));
		await vi.advanceTimersByTimeAsync(60_000);
		expect(api.played()).toEqual(['p1']);
		expect(await sender.pending()).toBe(1);
		expect(await dead()).toEqual([]);
	});

	it('waits as long as a 429 says before trying again', async () => {
		vi.useFakeTimers();
		api.next.push(() => ({
			status: 429,
			body: { status: 429, code: 'rate_limited' },
			retryAfter: 7
		}));
		const sender = open();
		sender.enqueue('play', report('p1'));
		await vi.advanceTimersByTimeAsync(6900);
		expect(api.played()).toEqual(['p1']);
		await vi.advanceTimersByTimeAsync(200);
		expect(api.played()).toEqual(['p1', 'p1']);
		expect(await sender.pending()).toBe(0);
	});

	it('waits from 1 s, doubling to 30 s, spread so devices do not return together', async () => {
		vi.useFakeTimers();
		/** The waits between tries when `Math.random` gives `random`. */
		async function waits(random: number) {
			env = memoryEnv();
			vi.spyOn(Math, 'random').mockReturnValue(random);
			const times: number[] = [];
			api.rules.answer = () => (times.push(Date.now()), { status: 500 });
			const sender = open();
			sender.enqueue('mark', mark('5', { liked: true }));
			await vi.advanceTimersByTimeAsync(100_000);
			sender.stop();
			return times.slice(1, 8).map((at, i) => at - times[i]);
		}
		expect(await waits(1)).toEqual([1000, 2000, 4000, 8000, 16_000, 30_000, 30_000]);
		// The least spread halves each.
		expect(await waits(0)).toEqual([500, 1000, 2000, 4000, 8000, 15_000, 15_000]);
	});

	it('drops the one play the server refuses, though the rest of its batch goes', async () => {
		const show = vi.spyOn(toaster, 'show');
		const refused = {
			status: 422,
			body: { status: 422, code: 'validation_failed', detail: 'No.' }
		};
		api.rules.answer = (r) =>
			(r.body.items as PlayReport[] | undefined)?.some((p) => p.playId === 'bad')
				? refused
				: { status: 204 };
		const sender = open();
		connectivity.setWorkOffline(true);
		for (const id of ['a', 'bad', 'c', 'd']) sender.enqueue('play', report(id));
		connectivity.setWorkOffline(false);
		await sender.idle();
		expect(await sender.pending()).toBe(0);
		expect(api.played().filter((id) => id === 'bad').length).toBeGreaterThan(1);
		expect(show.mock.calls.map(([message]) => message)).toEqual(['Couldn’t record a play. No.']);
		expect(await dead()).toEqual([]);
	});

	it(`sets a poison play aside after ${MAX_TRIES} tries, and says so once`, async () => {
		vi.useFakeTimers();
		const show = vi.spyOn(toaster, 'show');
		api.rules.answer = () => ({ status: 500 });
		const sender = open();
		sender.enqueue('play', report('poison'));
		for (let i = 0; i < MAX_TRIES + 5; i++) await vi.advanceTimersByTimeAsync(31_000);
		expect(api.played()).toHaveLength(MAX_TRIES);
		expect((await dead()).map((d) => [d.name, (d.args as PlayReport).playId])).toEqual([
			['play', 'poison']
		]);
		expect(await sender.pending()).toBe(0);
		expect(show.mock.calls.map(([message]) => message)).toEqual([
			'Couldn’t record a play. It kept failing, so it was set aside.'
		]);
	});

	it('sets a mutation aside once it is a week old, however few times it was tried', async () => {
		vi.useFakeTimers();
		api.rules.answer = () => ({ status: 500 });
		const sender = open();
		sender.enqueue('play', report('old'));
		await vi.advanceTimersByTimeAsync(100);
		vi.setSystemTime(Date.now() + MAX_AGE_MS + 1000);
		await vi.advanceTimersByTimeAsync(31_000);
		expect(api.played()).toHaveLength(2);
		expect((await dead()).length).toBe(1);
	});

	it('tells nothing of a refusal the feature tells itself, but still tells one set aside', async () => {
		vi.useFakeTimers();
		api.next.push(() => ({ status: 409, body: { status: 409, code: 'queue_changed' } }));
		const sender = open();
		const heard: { name: string; code?: string; dead?: boolean }[] = [];
		sender.onSettled((rec, _outcome, { code, dead }) => heard.push({ name: rec.name, code, dead }));
		sender.enqueue('editQueue', { editId: 'e1', queueId: 'q', edit: { type: 'next' } });
		await vi.advanceTimersByTimeAsync(100);
		expect(heard).toEqual([{ name: 'editQueue', code: 'queue_changed', dead: false }]);
		expect(toaster.current).toBeNull();

		api.rules.answer = () => ({ status: 500 });
		sender.enqueue('editQueue', { editId: 'e2', queueId: 'q', edit: { type: 'next' } });
		await vi.advanceTimersByTimeAsync(100);
		vi.setSystemTime(Date.now() + MAX_AGE_MS + 1000);
		// Its next try, a second at most after the first, finds it too old.
		await vi.advanceTimersByTimeAsync(1100);
		expect(heard.at(-1)).toMatchObject({ name: 'editQueue', dead: true });
		expect(toaster.current?.message).toMatch(/set aside/);
	});

	it('counts a kept queue the server no longer has as deleted', async () => {
		api.next.push(() => ({ status: 404, body: { status: 404, code: 'not_found' } }));
		const sender = open();
		const heard: string[] = [];
		sender.onSettled((_rec, outcome) => heard.push(outcome));
		sender.enqueue('deleteKeptQueue', { queueId: 'k' });
		await sender.idle();
		expect(api.lines()).toEqual(['DELETE /me/queues/k']);
		expect(heard).toEqual(['sent']);
		expect(toaster.current).toBeNull();
	});

	it('sets aside the edits made on one that kept failing, and takes them off the device', async () => {
		vi.useFakeTimers();
		api.rules.answer = (r) => (r.method === 'PATCH' ? { status: 500 } : { status: 204 });
		const sender = open();
		sender.enqueue('editPlaylist', edit(1, { title: 'One' }));
		sender.enqueue('editPlaylist', edit(2, { title: 'Two' }));
		sender.enqueue('mark', mark('5', { liked: true }));
		for (let i = 0; i < MAX_TRIES + 5; i++) await vi.advanceTimersByTimeAsync(31_000);
		expect((await dead()).map((d) => d.name)).toEqual(['editPlaylist', 'editPlaylist']);
		expect(device.rebased).toEqual(['11']);
		expect(api.sent('PUT')).toHaveLength(1);
	});

	it('does not count the failures of a server that does not answer', async () => {
		vi.useFakeTimers();
		const sender = open();
		const refuse = vi.fn(() => Promise.reject(new TypeError('Failed to fetch')));
		api.next.push(...Array.from({ length: 3 * MAX_TRIES }, () => refuse));
		sender.enqueue('play', report('kept'));
		for (let i = 0; i < MAX_TRIES + 10; i++) await vi.advanceTimersByTimeAsync(31_000);
		expect(refuse.mock.calls.length).toBeGreaterThan(MAX_TRIES);
		expect(await sender.pending()).toBe(1);
		expect(await dead()).toEqual([]);
	});

	it('does not count the gateway answers of a proxy in front of a server that is down', async () => {
		vi.useFakeTimers();
		let down = true;
		const ok = api.rules.answer;
		api.rules.answer = (r) => (down ? { status: 502 } : ok(r));
		const sender = open();
		sender.enqueue('play', report('kept'));
		for (let i = 0; i < MAX_TRIES + 10; i++) await vi.advanceTimersByTimeAsync(31_000);
		expect(api.played().length).toBeGreaterThan(MAX_TRIES);
		expect(await dead()).toEqual([]);
		down = false;
		await vi.advanceTimersByTimeAsync(31_000);
		expect(await sender.pending()).toBe(0);
		expect(await dead()).toEqual([]);
	});

	it('never lets a playlist that will not answer hold up the plays', async () => {
		api.next.push(() => new Promise<Reply>(() => {}));
		const sender = open();
		sender.enqueue('editPlaylist', edit(1, { title: 'Stuck' }));
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.enqueue('play', report('p1'));
		await vi.waitFor(() => expect(api.played()).toEqual(['p1']));
	});
});

describe('waking', () => {
	/** Has the page's global events reach the sender, as a browser's do. */
	function events() {
		const target = new EventTarget();
		vi.stubGlobal('addEventListener', target.addEventListener.bind(target));
		vi.stubGlobal('removeEventListener', target.removeEventListener.bind(target));
		return () => target.dispatchEvent(new Event('online'));
	}

	it('tries nothing while blocked, and at once when working offline is turned off', async () => {
		vi.useFakeTimers();
		const online = events();
		const sender = open();
		connectivity.setWorkOffline(true);
		sender.enqueue('play', report('p1'));
		await vi.advanceTimersByTimeAsync(5 * MAX_RETRY_MS);
		expect(api.requests).toEqual([]);
		// The browser says it is online again, with Work offline as it was: still nothing.
		online();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.requests).toEqual([]);
		connectivity.setWorkOffline(false);
		await vi.advanceTimersByTimeAsync(0);
		expect(api.played()).toEqual(['p1']);
	});

	it('goes again at once on the network coming back, not after its backoff', async () => {
		vi.useFakeTimers();
		const online = events();
		const refuse = vi.fn(() => Promise.reject(new TypeError('Failed to fetch')));
		// More refusals than can be tried in 4 s however the backoff is spread, so one is left.
		api.next.push(refuse, refuse, refuse, refuse, refuse);
		const sender = open();
		sender.enqueue('play', report('p1'));
		await vi.advanceTimersByTimeAsync(0);
		await vi.advanceTimersByTimeAsync(4000);
		const tried = api.requests.length;
		expect(tried).toBeGreaterThanOrEqual(2);
		online();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.requests.length).toBe(tried + 1);
	});

	it('goes again at once when told the server said hello, or the page shows again', async () => {
		vi.useFakeTimers();
		api.next.push(() => Promise.reject(new TypeError('Failed to fetch')));
		const sender = open();
		sender.enqueue('play', report('p1'));
		await vi.advanceTimersByTimeAsync(0);
		expect(api.requests).toHaveLength(1);
		sender.wake();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.played()).toEqual(['p1', 'p1']);
	});

	it('goes again at once when woken while a request that then fails was out', async () => {
		vi.useFakeTimers();
		let fail!: () => void;
		api.next.push(
			() => new Promise<Reply>((_, reject) => (fail = () => reject(new TypeError('Failed'))))
		);
		const sender = open();
		sender.enqueue('editQueue', { editId: 'e1', queueId: 'q', edit: { type: 'next' } });
		await vi.advanceTimersByTimeAsync(0);
		// The server says hello while the request it did not answer is still out.
		sender.wake();
		fail();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.lines()).toEqual(['POST /me/queue/edits', 'POST /me/queue/edits']);
		expect(await sender.pending()).toBe(0);
	});
});

describe('a change made while the one before is being sent', () => {
	it('is folded into it and sent after its answer, not deleted with it', async () => {
		let answer!: (reply: Reply) => void;
		api.next.push(() => new Promise<Reply>((resolve) => (answer = resolve)));
		const sender = open();
		const first = sender.enqueue('mark', mark('5', { liked: true, hidden: false }))!;
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		const second = sender.enqueue('mark', mark('5', { liked: false }))!;
		answer({ status: 204 });
		await sender.idle();
		expect(second.key).toBe(first.key);
		expect(api.lines()).toEqual([
			'PUT /libraries/1/tracks/5/like',
			'DELETE /libraries/1/tracks/5/like',
			// The like had unhidden it, which the folded change still says.
			'DELETE /libraries/1/tracks/5/hidden'
		]);
		expect(await sender.pending()).toBe(0);
	});

	it('is sent under a key of its own, which the answer to the one before cannot stand for', async () => {
		let answer!: (reply: Reply) => void;
		api.next.push(() => new Promise<Reply>((resolve) => (answer = resolve)));
		const sender = open();
		sender.enqueue('mark', mark('5', { liked: true, hidden: false }));
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.enqueue('mark', mark('5', { hidden: true, liked: false }));
		answer({ status: 204 });
		await sender.idle();
		expect(api.lines()).toEqual([
			'PUT /libraries/1/tracks/5/like',
			'PUT /libraries/1/tracks/5/hidden'
		]);
		const [like, hide] = api.requests;
		expect(hide.key).not.toBe(like.key);
	});
});

describe('a play reported again while its report is being sent', () => {
	it('stays one mutation, sent again with the later report once the first is answered', async () => {
		let answer!: (reply: Reply) => void;
		api.next.push(() => new Promise<Reply>((resolve) => (answer = resolve)));
		const sender = open();
		sender.enqueue('play', report('p1', 1000));
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.enqueue('play', report('p1', 5000));
		expect(
			await env
				.open('u')
				.all('mutations')
				.then((all) => all.length)
		).toBeLessThanOrEqual(1);
		answer({ status: 503 });
		await sender.flush();
		const reports = api.sent('POST').map((r) => (r.body.items as PlayReport[])[0].listenTimeMs);
		expect(reports).toEqual([1000, 5000]);
		expect(await sender.pending()).toBe(0);
	});
});

describe('two tabs', () => {
	it('send what a closed tab left once between them, and adopt it', async () => {
		connectivity.setWorkOffline(true);
		const gone = open();
		for (const id of ['p1', 'p2', 'p3']) gone.enqueue('play', report(id));
		gone.enqueue('mark', mark('5', { liked: true, hidden: false }));
		gone.enqueue('editPlaylist', edit(1, { title: 'One' }));
		await gone.idle();
		gone.stop();
		connectivity.setWorkOffline(false);

		const [a, b] = [open(), open()];
		await Promise.all([a.idle(), b.idle()]);
		expect(api.played().sort()).toEqual(['p1', 'p2', 'p3']);
		expect(api.sent('PUT')).toHaveLength(1);
		expect(api.sent('PATCH')).toHaveLength(1);
		expect(await a.pending()).toBe(0);
	});

	it('have the device make what a closed tab left, once each adopts it', async () => {
		connectivity.setWorkOffline(true);
		const gone = open();
		const queued = gone.enqueue('mark', mark('5', { liked: true }))!;
		await gone.idle();
		gone.stop();
		device.applied.length = 0;
		connectivity.setWorkOffline(false);
		const tab = open();
		await tab.idle();
		expect(device.applied.map((local) => local.calls.map((call) => call.id))).toEqual([
			[`${queued.key}/0`]
		]);
	});

	it('send what a closed tab left at once, though woken again while adopting it', async () => {
		connectivity.setWorkOffline(true);
		const gone = open();
		gone.enqueue('mark', mark('5', { hidden: false }));
		await gone.idle();
		gone.stop();
		connectivity.setWorkOffline(false);

		// A database that takes a moment to read, as a real one does.
		const quick = env;
		env = {
			...quick,
			open(user) {
				const port = quick.open(user);
				return {
					...port,
					all: async (store) => {
						await new Promise((done) => setTimeout(done, 10));
						return port.all(store);
					}
				};
			}
		};
		const tab = open();
		// The server's hello, as the page that opened the log is still adopting.
		tab.wake();
		await tab.idle();
		expect(api.sent('DELETE', '/tracks/5/hidden')).toHaveLength(1);
	});

	it('leave the mutations of a tab that is still open to it', async () => {
		connectivity.setWorkOffline(true);
		const busy = open();
		busy.enqueue('play', report('mine'));
		await busy.idle();
		connectivity.setWorkOffline(false);

		const other = open();
		await other.idle();
		// The busy tab sent its own on going online; the other sent none of it.
		await busy.idle();
		expect(api.played()).toEqual(['mine']);
	});

	it('leave a closed tab’s mutations to the tab that adopts them first', async () => {
		connectivity.setWorkOffline(true);
		const busy = open();
		busy.enqueue('play', report('mine'));
		await busy.idle();
		const other = open();
		connectivity.setWorkOffline(false);
		await other.idle();
		await busy.idle();
		// Closed, its mutations are the other's to adopt.
		busy.stop();
		other.wake();
		await other.idle();
		expect(api.played()).toEqual(['mine']);
	});

	it('send what a closed tab left of a scope before what this tab made after it', async () => {
		connectivity.setWorkOffline(true);
		const gone = open();
		gone.enqueue('editQueue', { editId: 'e1', queueId: 'q', edit: { type: 'next' } });
		await gone.idle();
		gone.stop();
		// Made later, as its sequence number says.
		await new Promise((resolve) => setTimeout(resolve, 2));
		const tab = open();
		tab.enqueue('startQueue', {
			body: {
				queueId: 'q2',
				context: { type: 'library' },
				source: { key: 'k', libraryId: '11', tracks: ['1'] },
				start: 0
			}
		});
		await tab.idle();
		connectivity.setWorkOffline(false);
		await tab.idle();

		expect(api.lines()).toEqual(['POST /me/queue/edits', 'POST /me/queue']);
	});

	it('count what every tab holds as unsent', async () => {
		connectivity.setWorkOffline(true);
		const a = open();
		const b = open();
		a.enqueue('play', report('p1'));
		b.enqueue('mark', mark('5', { liked: true }));
		b.enqueue('editPlaylist', edit(1, {}));
		b.enqueue('editPlaylist', edit(2, {}));
		await b.idle();
		expect(await a.pending()).toBe(4);
	});
});

describe('when the database fails', () => {
	it('sends anyway and says once that it was not kept', async () => {
		const show = vi.spyOn(toaster, 'show');
		vi.spyOn(console, 'error').mockImplementation(() => {});
		env = {
			...env,
			open: () => ({
				all: () => Promise.reject(new Error('full')),
				put: () => Promise.reject(new Error('full')),
				remove: () => Promise.reject(new Error('full')),
				claim: () => Promise.reject(new Error('full')),
				close() {}
			})
		};
		const sender = open();
		sender.enqueue('play', report('p1'));
		sender.enqueue('play', report('p2'));
		await sender.idle();
		expect(api.played()).toContain('p1');
		expect(show).toHaveBeenCalledTimes(1);
	});
});

describe('a mutation that outlives what it was sent from', () => {
	/** A request the test fails or answers when it likes. */
	function hang() {
		let settle!: { fail: () => void; ok: (reply?: Reply) => void };
		api.next.push(
			() =>
				new Promise<Reply>((resolve, reject) => {
					settle = {
						fail: () => reject(new TypeError('Failed to fetch')),
						ok: (reply = { status: 204 }) => resolve(reply)
					};
				})
		);
		return { fail: () => settle.fail(), ok: (reply?: Reply) => settle.ok(reply) };
	}

	it('ends with the server holding the latest mark after a restart', async () => {
		const first = hang();
		const sender = open();
		sender.enqueue('mark', mark('5', { liked: true, hidden: false }));
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.enqueue('mark', mark('5', { liked: false }));
		connectivity.setWorkOffline(true);
		first.fail();
		await sender.idle();
		sender.stop();
		connectivity.setWorkOffline(false);
		const next = open();
		await next.idle();
		expect(api.requests.at(-1)).toMatchObject({ method: 'DELETE' });
		expect(await next.pending()).toBe(0);
	});

	it('does not put one user’s play in the next user’s log, however its request ends', async () => {
		const first = hang();
		const sender = open('a');
		sender.enqueue('play', report('a-play'));
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.stop();
		sender.start('b', { env, fetch: api.fetch, device: device.device });
		first.fail();
		await sender.idle();
		expect(await env.open('b').all('mutations')).toEqual([]);
		expect(await sender.pending()).toBe(0);
	});

	it('does not send the rest of one user’s playlist edits as the next user’s', async () => {
		const first = hang();
		const sender = open('a');
		sender.enqueue('editPlaylist', edit(1, { title: 'One' }));
		sender.enqueue('editPlaylist', edit(2, { title: 'Two' }));
		await vi.waitFor(() => expect(api.sent('PATCH')).toHaveLength(1));
		sender.stop();
		sender.start('b', { env, fetch: api.fetch, device: device.device });
		first.ok({ status: 200, body: { ...playlist, version: 2 } });
		await sender.idle();
		expect(api.sent('PATCH')).toHaveLength(1);
		expect(await env.open('b').all('mutations')).toEqual([]);
	});

	it('does not tell a user of a mark that was sent after their log was let go', async () => {
		const first = hang();
		const sender = open();
		const heard = vi.fn();
		sender.onSettled(heard);
		const queued = sender.enqueue('mark', mark('5', { liked: true }))!;
		const outcome = sender.settled(queued.key);
		await vi.waitFor(() => expect(api.requests).toHaveLength(1));
		sender.stop();
		first.ok();
		expect(await outcome).toBe('kept');
		await new Promise((resolve) => setTimeout(resolve));
		expect(heard).not.toHaveBeenCalled();
	});
});

describe('keeping and sending', () => {
	it('saves what an edit’s answer carried with the next, so a reopened app sends it on that version', async () => {
		let release!: () => void;
		// The first edit is answered as usual, and the second is held.
		api.next.push(
			(r) => api.rules.answer(r),
			() => new Promise<Reply>((resolve) => (release = () => resolve({ status: 204 })))
		);
		// Another device had changed it, so the second edit's own base is out of date.
		api.state.version = 1;
		const sender = open();
		sender.enqueue('editPlaylist', edit(1, { title: 'One' }));
		sender.enqueue('editPlaylist', edit(7, { title: 'Two' }));
		await vi.waitFor(() => expect(api.sent('PATCH')).toHaveLength(2));
		expect(api.sent('PATCH')[1].body.baseVersion).toBe(2);
		await vi.waitFor(async () => {
			const [kept] = await env.open('u').all<Mutation>('mutations');
			expect(kept.carried).toMatchObject({ version: 2 });
		});
		release();
	});
});

describe('an append', () => {
	it('is sent after a reopen online, on whatever version the playlist has by then', async () => {
		connectivity.setWorkOffline(true);
		const before = open();
		before.enqueue('appendToPlaylist', append());
		await before.idle();
		before.stop();
		// Another device changed the playlist meanwhile, and the first look at it is out of date.
		api.state.version = 5;
		api.next.push(() => ({ status: 200, body: { ...playlist, version: 4 } }));
		connectivity.setWorkOffline(false);
		const after = open();
		await after.idle();
		const edits = api.sent('PATCH');
		expect(edits.map((r) => r.body.baseVersion)).toEqual([4, 5]);
		// The server keeps an answer only for an edit that applied, so the key is still free.
		expect(edits[0].key).toBe(edits[1].key);
		expect(edits[1].body.ops).toEqual([{ op: 'add', items: [{ type: 'track', id: '9' }] }]);
		expect(await after.pending()).toBe(0);
	});

	it('is not dropped for conflicts it can cure, and says what failed when refused', async () => {
		const show = vi.spyOn(toaster, 'show');
		api.next.push(() => ({ status: 200, body: { ...playlist, version: 1 } }));
		api.next.push(() => ({
			status: 422,
			body: { status: 422, code: 'validation_failed', detail: 'No.' }
		}));
		const sender = open();
		sender.enqueue('appendToPlaylist', append());
		await sender.idle();
		expect(show.mock.calls.map(([message]) => message)).toEqual([
			'Couldn’t add to “Road Trip”. No.'
		]);
		expect(await sender.pending()).toBe(0);
	});
});

/** A deletion of playlist `p1`. */
const deletion = { libraryId: '11', playlistId: 'p1', title: 'Road Trip' };

describe('a mutation held until its time', () => {
	it('is not sent before then, whatever wakes the sender, and goes once it is due', async () => {
		vi.useFakeTimers();
		const sender = open();
		sender.enqueue('deletePlaylist', deletion, { notBefore: Date.now() + 5000 });
		await vi.advanceTimersByTimeAsync(4000);
		sender.wake();
		await sender.flush();
		expect(api.requests).toEqual([]);
		expect(device.applied.map((local) => local.calls[0].name)).toEqual(['deletePlaylist']);
		await vi.advanceTimersByTimeAsync(1100);
		expect(api.lines()).toEqual(['DELETE /libraries/11/playlists/p1']);
		expect(api.requests[0].key).toBeTruthy();
		expect(await sender.pending()).toBe(0);
	});

	it('follows the edits made before it, and counts one the server no longer has as sent', async () => {
		vi.useFakeTimers();
		api.next.push(() => ({ status: 200, body: { ...playlist, version: 2 } }));
		api.next.push(() => ({ status: 404, body: { status: 404, code: 'not_found' } }));
		const sender = open();
		sender.enqueue('editPlaylist', edit(1, { title: 'Trip' }));
		sender.enqueue('deletePlaylist', deletion, { notBefore: Date.now() + 1000 });
		await vi.advanceTimersByTimeAsync(2000);
		expect(api.lines()).toEqual([
			'PATCH /libraries/11/playlists/p1',
			'DELETE /libraries/11/playlists/p1'
		]);
		expect(await sender.pending()).toBe(0);
		expect(toaster.current).toBeNull();
	});

	it('is sent by another tab when the one that made it closed first', async () => {
		vi.useFakeTimers();
		const before = open();
		before.enqueue('deletePlaylist', deletion, { notBefore: Date.now() + 5000 });
		await before.idle();
		before.stop();
		const after = open();
		await vi.advanceTimersByTimeAsync(4000);
		expect(api.requests).toEqual([]);
		await vi.advanceTimersByTimeAsync(1100);
		expect(api.lines()).toEqual(['DELETE /libraries/11/playlists/p1']);
		expect(await after.pending()).toBe(0);
	});
});

describe('cancelling an unsent mutation', () => {
	it('takes it out of the log, has the device take its change back, and says so', async () => {
		vi.useFakeTimers();
		const sender = open();
		const { key } = sender.enqueue('deletePlaylist', deletion, { notBefore: Date.now() + 5000 })!;
		const settled = sender.settled(key);
		expect(await sender.cancel(key)).toBe(true);
		expect(await settled).toBe('cancelled');
		expect(device.rebased).toEqual(['11']);
		expect(await sender.pending()).toBe(0);
		await vi.advanceTimersByTimeAsync(10_000);
		expect(api.requests).toEqual([]);
		expect(await sender.cancel(key)).toBe(false);
	});

	it('cannot take back one that was sent, or whose request is under its way', async () => {
		const sender = open();
		const sentKey = sender.enqueue('deletePlaylist', deletion)!.key;
		await sender.idle();
		expect(await sender.cancel(sentKey)).toBe(false);

		let answer!: () => void;
		api.next.push(() => new Promise((resolve) => (answer = () => resolve({ status: 204 }))));
		const key = sender.enqueue('deletePlaylist', { ...deletion, playlistId: 'p2' })!.key;
		await vi.waitFor(() => expect(api.requests).toHaveLength(2));
		expect(await sender.cancel(key)).toBe(false);
		answer();
		await sender.idle();
		expect(await sender.pending()).toBe(0);
		expect(device.rebased).toEqual([]);
	});
});

describe('a playlist created', () => {
	it('is sent under the ID and key it was made with, and its edits follow on the version it made', async () => {
		api.next.push(() => ({ status: 201, body: { ...playlist, id: '77', version: 1 } }));
		const sender = open();
		sender.enqueue('createPlaylist', {
			libraryId: '11',
			id: '77',
			title: 'New',
			items: [{ type: 'album', id: '3' }]
		});
		sender.enqueue('editPlaylist', { ...edit(9, { title: 'Newer' }), playlistId: '77' });
		await sender.idle();
		const [create, patch] = api.requests;
		expect(create).toMatchObject({
			method: 'POST',
			path: '/libraries/11/playlists',
			body: { id: '77', title: 'New', items: [{ type: 'album', id: '3' }] }
		});
		expect(create.key).toBeTruthy();
		expect(patch).toMatchObject({ method: 'PATCH', body: { baseVersion: 1, title: 'Newer' } });
		// The server expands an album; nothing was made on the device for what it cannot expand.
		expect(device.applied.map((local) => local.calls.map((c) => c.name))).toEqual([
			['editPlaylist']
		]);
	});

	it('is made on the device with the tracks given, even before it is sent', async () => {
		connectivity.setWorkOffline(true);
		const sender = open();
		sender.enqueue('createPlaylist', {
			libraryId: '11',
			id: '77',
			title: 'New',
			items: [{ type: 'track', id: '5' }],
			tracks: ['5']
		});
		await sender.idle();
		expect(device.applied[0].calls[0]).toMatchObject({
			name: 'createPlaylist',
			args: { id: '77', title: 'New', items: ['5'] }
		});
		expect(api.requests).toEqual([]);
		expect(await sender.pending()).toBe(1);
	});

	it('is refused with its edits taken back when its ID is taken', async () => {
		api.next.push(() => ({ status: 409, body: { status: 409, code: 'id_taken' } }));
		const sender = open();
		sender.enqueue('createPlaylist', { libraryId: '11', id: '77', title: 'New' });
		await sender.idle();
		expect(toaster.current?.message).toMatch(/^Couldn’t create “New”/);
		expect(device.rebased).toEqual(['11']);
		expect(await sender.pending()).toBe(0);
	});
});
