import { describe, expect, it, vi } from 'vitest';
import type { Playlist } from './api/types';

// A device whose copy of the playlist is at version 1, behind the server's version 5.
const device = vi.hoisted(() => ({ onDevice: vi.fn(() => true), query: vi.fn() }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog: device }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { offline: false } }));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import { addTo } from './adding.svelte';
import { EditablePlaylist } from './playlist.svelte';

const playlist = {
	id: 'p1',
	libraryId: '11',
	title: 'Road Trip',
	version: 1,
	trackCount: 0,
	durationUs: 0
} as unknown as Playlist;

device.query.mockImplementation(async (_library: string, name: string) =>
	JSON.stringify(
		name === 'listPlaylistItems'
			? { items: [], nextCursor: null, total: 0, version: 1 }
			: { ...playlist, version: 1 }
	)
);

/** A server at version 5 that refuses an edit based on any other version, and records every request. */
function server(serverItems: unknown[] = []) {
	let version = 5;
	const requests: { method: string; path: string; body: Record<string, unknown> }[] = [];
	const fetch = async (input: RequestInfo | URL, init: RequestInit = {}) => {
		const path = new URL(String(input), 'http://test').pathname.replace(/^.*\/api\/v1/, '');
		const method = init.method ?? 'GET';
		const body = init.body ? JSON.parse(String(init.body)) : {};
		requests.push({ method, path, body });
		if (method === 'GET' && path.endsWith('/items'))
			return Response.json({ items: serverItems, nextCursor: null, total: 0, version });
		if (method === 'GET') return Response.json({ ...playlist, version });
		if (body.baseVersion !== version)
			return Response.json({ status: 409, code: 'version_conflict' }, { status: 409 });
		return Response.json({ ...playlist, version: ++version });
	};
	return { requests, fetch: fetch as typeof globalThis.fetch };
}

describe('a device behind the server', () => {
	it('the first edit is based on the server’s version and is not dropped', async () => {
		const api = server();
		const list = new EditablePlaylist(api.fetch, { playlist, items: [] });
		list.edit({ title: 'Mine' });
		await list.idle();
		list.edit({ title: 'Mine again' });
		await list.idle();
		const edits = api.requests.filter((r) => r.method === 'PATCH');
		expect(edits.map((e) => e.body.baseVersion)).toEqual([5, 6]);
		expect(list.playlist.version).toBe(7);
	});

	it('an edit conflicts when the server’s items are not the ones its positions name', async () => {
		const api = server([{ track: { id: '9' } }]);
		const list = new EditablePlaylist(api.fetch, { playlist, items: [] });
		list.edit({ title: 'Mine' });
		await list.idle();
		expect(api.requests.some((r) => r.method === 'PATCH')).toBe(false);
		expect(list.items).toEqual([{ track: { id: '9' } }]);
	});

	it('adding to a playlist sends the server’s version after one read of it', async () => {
		const api = server();
		await addTo(api.fetch, playlist, {
			label: 'a song',
			refs: [{ type: 'track', id: '9' }]
		} as never);
		expect(api.requests.map((r) => `${r.method} ${r.path}`)).toEqual([
			'GET /libraries/11/playlists/p1',
			'PATCH /libraries/11/playlists/p1'
		]);
		expect(api.requests[1].body.baseVersion).toBe(5);
	});
});
