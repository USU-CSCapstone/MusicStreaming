import { afterEach, beforeEach, expect, it, vi } from 'vitest';

// The device holds library 1, and its copy still says track 5 is liked: the catch-up the
// connection starts has not committed yet.
const queried = vi.hoisted(() => [] as string[]);
vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: {
		outdated: false,
		onDevice: () => true,
		waitForDevice: async () => true,
		indexed: () => true,
		mutate: async () => {},
		rebase: async () => {},
		confirmed: async () => {},
		query: async (_library: string, name: string) => {
			queried.push(name);
			const stale = { id: '5', title: 'Signal', liked: true };
			const items = name === 'listHiddenTracks' ? [] : [stale];
			return JSON.stringify({ items, nextCursor: null, total: items.length });
		}
	}
}));

const { marks } = await import('./marks.svelte');
const { sender } = await import('./sync/sender');
const { memoryEnv } = await import('./sync/store');

const track = { id: '5', title: 'Signal', liked: false };
let requests: string[];

/** The server, which holds track 5 as not liked and nothing hidden. */
const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
	const url = new URL(String(input), 'http://test');
	const method = init.method ?? 'GET';
	requests.push(`${method} ${url.pathname.replace(/^.*\/api\/v1/, '')}`);
	if (method !== 'GET') return new Response(null, { status: 204 });
	const ids = url.pathname.endsWith('/hidden-tracks')
		? []
		: (url.searchParams.get('ids')?.split(',') ?? []);
	const items = ids.map((id) => ({ ...track, id }));
	return Response.json({ items, nextCursor: null, total: items.length });
}) as typeof globalThis.fetch;

beforeEach(async () => {
	marks.clear();
	sender.start('1', { env: memoryEnv(), fetch });
	requests = [];
	queried.length = 0;
});

afterEach(() => sender.stop());

it('takes the server’s word at reconnection, not the device copy its catch-up has yet to bring up to date', async () => {
	// Liked here, then unliked on another device while this one was disconnected.
	await marks.like('1', track, true);
	requests = [];
	await marks.receive(
		{ type: 'hello', deviceId: '1', serverTime: '2026-01-01T00:00:00.000Z' },
		fetch
	);
	expect(queried).toEqual([]);
	expect(requests.toSorted()).toEqual([
		'GET /libraries/1/hidden-tracks',
		'GET /libraries/1/tracks'
	]);
	expect(marks.liked('1', { ...track, liked: false })).toBe(false);
});
