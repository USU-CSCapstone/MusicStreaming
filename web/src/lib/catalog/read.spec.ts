import { beforeEach, describe, expect, it, vi } from 'vitest';

const device = vi.hoisted(() => ({
	onDevice: vi.fn(),
	query: vi.fn()
}));
const link = vi.hoisted(() => ({ offline: false }));
vi.mock('./client.svelte', () => ({ catalog: device }));
vi.mock('$lib/realtime.svelte', () => ({
	realtime: {
		get offline() {
			return link.offline;
		}
	}
}));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

import { ApiError } from '$lib/api/root';
import { read, readSearch, unavailableIfOffline } from './read';

const ON_DEVICE = { items: ['device'] };
const ON_SERVER = { items: ['server'] };

/** A read of albums of library 1 with the server answering `answer`, and what it was asked. */
function albums(params: object = { sort: 'name' }, answer: unknown = ON_SERVER) {
	const server = vi.fn(async () => answer);
	return { server, result: read('listAlbums', '1', params, server) };
}

beforeEach(() => {
	vi.clearAllMocks();
	vi.spyOn(console, 'error').mockImplementation(() => {});
	link.offline = false;
	device.onDevice.mockResolvedValue(true);
	device.query.mockResolvedValue(JSON.stringify(ON_DEVICE));
});

describe('where a read is answered', () => {
	it('a library never fully synced is read from the server', async () => {
		device.onDevice.mockResolvedValue(false);
		const { server, result } = albums();
		expect(await result).toEqual(ON_SERVER);
		expect(server).toHaveBeenCalledOnce();
		expect(device.query).not.toHaveBeenCalled();
	});

	it('a library never fully synced is still read from the server while only the realtime connection is down', async () => {
		device.onDevice.mockResolvedValue(false);
		link.offline = true;
		const { server, result } = albums();
		expect(await result).toEqual(ON_SERVER);
		expect(server).toHaveBeenCalledOnce();
	});

	it('a library never fully synced says so when the server cannot be reached either, and rethrows when online', async () => {
		device.onDevice.mockResolvedValue(false);
		const failed = new TypeError('Failed to fetch');
		await expect(read('listAlbums', '1', {}, () => Promise.reject(failed))).rejects.toBe(failed);
		link.offline = true;
		await expect(read('listAlbums', '1', {}, () => Promise.reject(failed))).rejects.toMatchObject({
			status: 503,
			code: 'not_on_device'
		});
		const unreachable = new ApiError(503, 'unavailable_offline', 'unreachable');
		await expect(
			read('listAlbums', '1', {}, () => Promise.reject(unreachable))
		).rejects.toMatchObject({ code: 'not_on_device' });
		const answered = new ApiError(401, 'unauthorized', 'signed out');
		await expect(read('listAlbums', '1', {}, () => Promise.reject(answered))).rejects.toBe(
			answered
		);
	});

	it('a current library is read from the device alone, with the page’s parameters', async () => {
		const { server, result } = albums({ sort: 'name', limit: 5 });
		expect(await result).toEqual(ON_DEVICE);
		expect(device.query).toHaveBeenCalledWith('1', 'listAlbums', { sort: 'name', limit: 5 }, false);
		expect(server).not.toHaveBeenCalled();
	});

	it('a library on the device answers from it, without asking the server', async () => {
		const { server, result } = albums();
		expect(await result).toEqual(ON_DEVICE);
		await Promise.resolve();
		expect(server).not.toHaveBeenCalled();
	});

	it('the browser saying it is offline counts, whatever the realtime connection says', async () => {
		vi.stubGlobal('navigator', { onLine: false });
		try {
			device.onDevice.mockResolvedValue(false);
			const { server, result } = albums();
			await expect(result).rejects.toMatchObject({ code: 'not_on_device' });
			expect(server).not.toHaveBeenCalled();
		} finally {
			vi.unstubAllGlobals();
		}
	});

	it('offline reads the device, even behind or mid-update', async () => {
		link.offline = true;
		const { server, result } = albums();
		expect(await result).toEqual(ON_DEVICE);
		expect(server).not.toHaveBeenCalled();
		// Offline, the device waits for an update under way rather than failing at once.
		expect(device.query).toHaveBeenCalledWith('1', 'listAlbums', expect.anything(), true);
	});

	it('a device that fails answers from the server when online, never as an error', async () => {
		device.query.mockRejectedValue(new Error('disk I/O error'));
		const { server, result } = albums();
		expect(await result).toEqual(ON_SERVER);
		expect(server).toHaveBeenCalledOnce();
	});

	it('a device that fails offline says the page is unavailable', async () => {
		device.query.mockRejectedValue(new Error('disk I/O error'));
		link.offline = true;
		const { server, result } = albums();
		await expect(result).rejects.toMatchObject({ status: 503, code: 'unavailable_offline' });
		expect(server).not.toHaveBeenCalled();
	});

	it('a get the device finds nothing for is the server’s to answer, and not found offline', async () => {
		device.query.mockResolvedValue('null');
		const { server, result } = albums();
		expect(await result).toEqual(ON_SERVER);
		expect(server).toHaveBeenCalledOnce();

		link.offline = true;
		await expect(albums().result).rejects.toMatchObject({ status: 404 });
	});
});

describe('unavailableIfOffline', () => {
	it('marks a failure unavailable offline, and rethrows it online', () => {
		const error = new TypeError('Failed to fetch');
		expect(() => unavailableIfOffline(error)).toThrow(error);
		link.offline = true;
		expect(() => unavailableIfOffline(error)).toThrowError(
			expect.objectContaining({ status: 503, code: 'unavailable_offline' })
		);
	});
});

describe('readSearch', () => {
	/** Runs `readSearch` for library 1 against `server`, which answers `ON_SERVER` unless given another. */
	const search = (server = vi.fn(async () => ON_SERVER)) => ({
		server,
		result: readSearch('search', '1', { q: 'a' }, server)
	});

	it('asks the server first when it can be reached', async () => {
		const { result } = search();
		expect(await result).toEqual(ON_SERVER);
		expect(device.query).not.toHaveBeenCalled();
	});

	it('offline answers from the device without asking the server', async () => {
		link.offline = true;
		const { server, result } = search();
		expect(await result).toEqual(ON_DEVICE);
		expect(device.query).toHaveBeenCalledWith('1', 'search', { q: 'a' }, true);
		expect(server).not.toHaveBeenCalled();
	});

	it.each([new TypeError('Failed to fetch'), new ApiError(502, undefined, 'bad gateway')])(
		'a server that fails is answered by the device: %s',
		async (failure) => {
			const { result } = search(vi.fn().mockRejectedValue(failure));
			expect(await result).toEqual(ON_DEVICE);
		}
	);

	it('a server that refuses the request is not second-guessed', async () => {
		const refusal = new ApiError(400, 'invalid', 'no');
		await expect(search(vi.fn().mockRejectedValue(refusal)).result).rejects.toBe(refusal);
		expect(device.query).not.toHaveBeenCalled();
	});

	it('a library not on the device asks the server while only the realtime connection is down, says so when that fails or the device is offline, and online keeps the server’s failure', async () => {
		device.onDevice.mockResolvedValue(false);
		const failure = new TypeError('Failed to fetch');
		await expect(search(vi.fn().mockRejectedValue(failure)).result).rejects.toBe(failure);
		link.offline = true;
		const { server, result } = search();
		expect(await result).toEqual(ON_SERVER);
		expect(server).toHaveBeenCalledOnce();
		await expect(search(vi.fn().mockRejectedValue(failure)).result).rejects.toMatchObject({
			code: 'not_on_device'
		});
		vi.stubGlobal('navigator', { onLine: false });
		try {
			const known = search();
			await expect(known.result).rejects.toMatchObject({ code: 'not_on_device' });
			expect(known.server).not.toHaveBeenCalled();
		} finally {
			vi.unstubAllGlobals();
		}
	});
});
