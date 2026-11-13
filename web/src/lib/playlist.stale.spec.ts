import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Playlist } from './api/types';

// A device whose copy of the playlist is at version 1, behind the server's version 5.
const device = vi.hoisted(() => ({ onDevice: vi.fn(() => true), query: vi.fn() }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog: device }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { offline: false } }));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import { addTo } from './adding.svelte';
import { CONFLICT_MESSAGE, EditablePlaylist } from './playlist.svelte';
import type { Device } from './sync/device';
import { sender } from './sync/sender';
import { memoryEnv } from './sync/store';
import { toaster } from './toast.svelte';

/** A device that makes nothing: what it shows is the mocked catalog's. */
const nothing: Device = {
	apply: async () => {},
	rebase: async () => {},
	confirmed: async () => {}
};

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

/**
 * A server at version 5 that refuses an edit based on any other version, and records every request.
 */
function server(serverItems: unknown[] = []) {
	let version = 5;
	const requests: { method: string; path: string; body: Record<string, unknown> }[] = [];
	/**
	 * Answers the playlist API at the server's version, refusing edits based on another, and records
	 * each request.
	 */
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

afterEach(() => {
	sender.stop();
	toaster.dismiss();
});

describe('a device behind the server', () => {
	it('an edit goes on the server’s version where the items are the ones it was made on', async () => {
		const api = server();
		sender.start('u', { env: memoryEnv(), fetch: api.fetch, device: nothing });
		const list = new EditablePlaylist(api.fetch, { playlist, items: [] });
		list.edit({ title: 'Mine' });
		await list.idle();
		list.edit({ title: 'Mine again' });
		await list.idle();
		const edits = api.requests.filter((r) => r.method === 'PATCH');
		// Each is refused at the version the device gave, checked, and sent at the server's.
		expect(edits.map((e) => e.body.baseVersion)).toEqual([1, 5, 2, 6]);
		expect(toaster.current).toBeNull();
	});

	it('an edit is refused when the server’s items are not the ones its positions name', async () => {
		const api = server([{ track: { id: '9' } }]);
		sender.start('u', { env: memoryEnv(), fetch: api.fetch, device: nothing });
		const list = new EditablePlaylist(api.fetch, { playlist, items: [] });
		list.edit({ title: 'Mine' });
		await list.idle();
		expect(api.requests.filter((r) => r.method === 'PATCH')).toHaveLength(1);
		expect(toaster.current?.message).toBe(CONFLICT_MESSAGE);
		// Shown as the device reads it again.
		await vi.waitFor(() => expect(list.playlist.title).toBe('Road Trip'));
	});

	it('adding to a playlist sends the server’s version after one read of it', async () => {
		const api = server();
		sender.start('u', { env: memoryEnv(), fetch: api.fetch, device: nothing });
		await addTo(api.fetch, playlist, {
			label: 'a song',
			refs: [{ type: 'track', id: '9' }],
			tracks: async () => [{ id: '9' }]
		} as never);
		await sender.idle();
		expect(api.requests.map((r) => `${r.method} ${r.path}`)).toEqual([
			'GET /libraries/11/playlists/p1',
			'PATCH /libraries/11/playlists/p1'
		]);
		expect(api.requests[1].body.baseVersion).toBe(5);
	});
});
