import { describe, expect, it, vi } from 'vitest';

const device = vi.hoisted(() => ({
	onDevice: vi.fn(),
	waitForDevice: vi.fn(),
	query: vi.fn()
}));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog: device }));
const link = vi.hoisted(() => ({ working: false, offline: false, unreachable: false }));
vi.mock('$lib/connectivity.svelte', () => ({
	connectivity: {
		/** Where the device stands, as the test sets it. */
		get state() {
			return link.working
				? 'working-offline'
				: link.offline
					? 'offline'
					: link.unreachable
						? 'unreachable'
						: 'online';
		},
		get blocked() {
			return link.working || link.offline;
		},
		lose: () => (link.unreachable = true),
		reach: () => (link.unreachable = false)
	}
}));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

import { getPlaylist, getTracksByIds, listKeptQueues, listPlaylistItems, logout } from './client';

/**
 * Says whether the library is on the device, to a read that waits for the owner and to one that
 * does not.
 */
function has(value: boolean) {
	device.onDevice.mockReturnValue(value);
	device.waitForDevice.mockResolvedValue(value);
}

/** A server whose playlist is at version 6 while the device, not yet caught up, holds version 5. */
function stale() {
	vi.clearAllMocks();
	has(true);
	device.query.mockResolvedValue(JSON.stringify({ version: 5 }));
	return vi.fn(async () => Response.json({ version: 6 }));
}

describe('reads that an edit is based on', () => {
	it('a playlist page reads the device’s version, and an edit’s base version the server’s', async () => {
		const f = stale();
		expect(await getPlaylist(f, '1', 'p')).toEqual({ version: 5 });
		expect(f).not.toHaveBeenCalled();
		expect(await getPlaylist(f, '1', 'p', true)).toEqual({ version: 6 });
		expect(f).toHaveBeenCalledOnce();
	});

	it('the items a reload after a conflict lists come from the server', async () => {
		const f = stale();
		expect(await listPlaylistItems(f, '1', 'p', {}, true)).toEqual({ version: 6 });
		expect(device.query).not.toHaveBeenCalled();
	});
});

describe('tracks looked up by ID', () => {
	/** A device that holds only track 1 of the two asked for, and a server that holds both. */
	function behind() {
		vi.clearAllMocks();
		has(true);
		device.query.mockResolvedValue(JSON.stringify({ items: [{ id: '1' }] }));
		return vi.fn(async () => Response.json({ items: [{ id: '1' }, { id: '2' }] }));
	}

	it('go to the server when the device answers fewer than asked while online', async () => {
		const f = behind();
		expect((await getTracksByIds(f, '1', ['1', '2'])).items).toHaveLength(2);
		expect(f).toHaveBeenCalledOnce();
	});

	it('come from the device when it answers them all', async () => {
		const f = behind();
		expect((await getTracksByIds(f, '1', ['1'])).items).toEqual([{ id: '1' }]);
		expect(f).not.toHaveBeenCalled();
	});

	it('are not asked of the server twice when the library is not on the device', async () => {
		const f = behind();
		has(false);
		await getTracksByIds(f, '1', ['1', '2', '3']);
		expect(f).toHaveBeenCalledOnce();
	});

	it('are topped up from the server while only the realtime connection is down, and left short when it fails too', async () => {
		link.unreachable = true;
		try {
			const f = behind();
			expect((await getTracksByIds(f, '1', ['1', '2'])).items).toHaveLength(2);
			const down = vi.fn(async () => Promise.reject(new TypeError('Failed to fetch')));
			expect((await getTracksByIds(down, '1', ['1', '2'])).items).toHaveLength(1);
		} finally {
			link.unreachable = false;
		}
	});
});

describe('what a request says of the server', () => {
	it('an answer below 500 reaches it, a 5xx does not, and a failure to get one loses it', async () => {
		link.unreachable = true;
		await listKeptQueues(vi.fn(async () => new Response('bad gateway', { status: 502 }))).catch(
			() => {}
		);
		expect(link.unreachable).toBe(true);
		await listKeptQueues(vi.fn(async () => Response.json({ items: [] })));
		expect(link.unreachable).toBe(false);
		await expect(
			listKeptQueues(vi.fn().mockRejectedValue(new TypeError('x')))
		).rejects.toMatchObject({ code: 'unavailable_offline' });
		expect(link.unreachable).toBe(true);
	});

	it.each(['working', 'offline'] as const)(
		'a request the device does not send while %s changes nothing',
		async (why) => {
			link.unreachable = false;
			link[why] = true;
			const f = vi.fn();
			await listKeptQueues(f).catch(() => {});
			expect(f).not.toHaveBeenCalled();
			expect(link.unreachable).toBe(false);
			link[why] = false;
		}
	);
});

describe('working offline', () => {
	it('sends no request: a read is unavailable offline, a write fails as with no network', async () => {
		link.working = true;
		const f = vi.fn(async () => Response.json({}));
		await expect(listKeptQueues(f)).rejects.toMatchObject({
			status: 503,
			code: 'unavailable_offline'
		});
		await expect(logout(f)).rejects.toBeInstanceOf(TypeError);
		expect(f).not.toHaveBeenCalled();
		link.working = false;
		await listKeptQueues(f);
		expect(f).toHaveBeenCalledOnce();
	});
});
