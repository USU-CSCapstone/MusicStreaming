import { describe, expect, it, vi } from 'vitest';

const device = vi.hoisted(() => ({ onDevice: vi.fn(), query: vi.fn() }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog: device }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { offline: false } }));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

import { getPlaylist, getTracksByIds, listPlaylistItems } from './client';

/** A server whose playlist is at version 6 while the device, not yet caught up, holds version 5. */
function stale() {
	vi.clearAllMocks();
	device.onDevice.mockResolvedValue(true);
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
		device.onDevice.mockResolvedValue(true);
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
		device.onDevice.mockResolvedValue(false);
		await getTracksByIds(f, '1', ['1', '2', '3']);
		expect(f).toHaveBeenCalledOnce();
	});
});
