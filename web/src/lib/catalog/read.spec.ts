import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const device = vi.hoisted(() => ({
	onDevice: vi.fn(),
	indexed: vi.fn(),
	query: vi.fn()
}));
const link = vi.hoisted(() => ({ offline: false, working: false }));
vi.mock('./client.svelte', () => ({ catalog: device }));
vi.mock('$lib/realtime.svelte', () => ({
	realtime: {
		/** Whether the realtime connection is down, as the test sets it. */
		get offline() {
			return link.offline;
		}
	}
}));
vi.mock('$lib/connectivity.svelte', () => ({
	connectivity: {
		/** Whether the device is set to work offline, as the test sets it. */
		get workOffline() {
			return link.working;
		}
	}
}));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

import { ApiError } from '$lib/api/root';
import { SLOW_MS, offline, read, readSearch, unavailableIfOffline } from './read';

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
	link.working = false;
	device.onDevice.mockResolvedValue(true);
	device.indexed.mockResolvedValue(true);
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

	it('a library never fully synced says so when the server cannot be reached either, and lets a refusal stand', async () => {
		device.onDevice.mockResolvedValue(false);
		const failed = new TypeError('Failed to fetch');
		await expect(read('listAlbums', '1', {}, () => Promise.reject(failed))).rejects.toMatchObject({
			code: 'not_on_device'
		});
		const refused = new ApiError(401, 'unauthenticated', 'sign in');
		await expect(read('listAlbums', '1', {}, () => Promise.reject(refused))).rejects.toBe(refused);
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

describe('a server that cannot be reached while no owner is serving yet', () => {
	it('is answered from the device, which is waited for as offline', async () => {
		// Not asked of the owner at first, since none serves; found once it does.
		device.onDevice.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
		const failed = new TypeError('Failed to fetch');
		expect(await read('listAlbums', '1', { sort: 'name' }, () => Promise.reject(failed))).toEqual(
			ON_DEVICE
		);
		expect(device.onDevice).toHaveBeenLastCalledWith('1', true);
		expect(device.query).toHaveBeenCalledWith('1', 'listAlbums', { sort: 'name' }, true);
	});

	it('a library not on the device shows the server’s own 500 while online', async () => {
		device.onDevice.mockResolvedValue(false);
		const broken = new ApiError(500, 'internal', 'broken');
		await expect(read('listAlbums', '1', {}, () => Promise.reject(broken))).rejects.toBe(broken);
	});

	it('says unavailable when the device then fails, as offline', async () => {
		device.onDevice.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
		device.query.mockRejectedValue(new Error('closed'));
		await expect(
			read('listAlbums', '1', {}, () => Promise.reject(new TypeError('Failed to fetch')))
		).rejects.toMatchObject({ code: 'unavailable_offline' });
	});
});

describe('offline', () => {
	it('is true while working offline, whatever the connection says', () => {
		expect(offline()).toBe(false);
		link.working = true;
		expect(offline()).toBe(true);
	});

	it('working offline reads the device and never asks the server', async () => {
		link.working = true;
		const { server, result } = albums();
		expect(await result).toEqual(ON_DEVICE);
		expect(server).not.toHaveBeenCalled();
		const search = vi.fn(async () => ON_SERVER);
		expect(await readSearch('search', '1', { q: 'a' }, search)).toEqual(ON_DEVICE);
		expect(search).not.toHaveBeenCalled();
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

	it('a server that cannot be reached is answered by a device that only a waiting ask finds, as no owner is serving yet', async () => {
		device.onDevice.mockImplementation(async (_library: string, wait?: boolean) => !!wait);
		const { result } = search(vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));
		expect(await result).toEqual(ON_DEVICE);
		device.onDevice.mockReset();
	});

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
		const refused = new ApiError(401, 'unauthenticated', 'sign in');
		await expect(search(vi.fn().mockRejectedValue(refused)).result).rejects.toBe(refused);
		vi.stubGlobal('navigator', { onLine: false });
		try {
			const known = search();
			await expect(known.result).rejects.toMatchObject({ code: 'not_on_device' });
			expect(known.server).not.toHaveBeenCalled();
		} finally {
			vi.unstubAllGlobals();
		}
	});

	describe('a slow server', () => {
		/** A server answer the test settles itself. */
		const hanging = () => {
			let answer!: (value: unknown) => void;
			let fail!: (error: unknown) => void;
			const server = vi.fn(
				() =>
					new Promise<unknown>((resolve, reject) => {
						answer = resolve;
						fail = reject;
					})
			);
			return { server, answer: (v: unknown) => answer(v), fail: (e: unknown) => fail(e) };
		};

		beforeEach(() => vi.useFakeTimers());
		afterEach(() => vi.useRealTimers());

		it('hands off to the device after the wait when the library is on it with its index built', async () => {
			const { server } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS - 1);
			expect(device.query).not.toHaveBeenCalled();
			await vi.advanceTimersByTimeAsync(1);
			expect(await result).toEqual(ON_DEVICE);
		});

		it('a server that answers within the wait is the answer, and the device is never asked', async () => {
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS - 1);
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			expect(device.query).not.toHaveBeenCalled();
		});

		it('a late failure after the hand-off is not an unhandled rejection', async () => {
			const { server, fail } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			expect(await result).toEqual(ON_DEVICE);
			fail(new TypeError('Failed to fetch'));
			await vi.advanceTimersByTimeAsync(0);
		});

		it('with the library not on the device it keeps waiting for the server', async () => {
			device.onDevice.mockResolvedValue(false);
			device.indexed.mockResolvedValue(false);
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS * 4);
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
			expect(device.query).not.toHaveBeenCalled();
		});

		it('with the index not built it keeps waiting for the server, rather than building it for a server that is only slow', async () => {
			device.indexed.mockResolvedValue(false);
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS * 4);
			expect(device.indexed).toHaveBeenCalledWith('1');
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
			expect(device.query).not.toHaveBeenCalled();
		});

		it('a server that answers while the device is asked about its index is not kept waiting', async () => {
			device.indexed.mockReturnValue(new Promise(() => {}));
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
			expect(device.query).not.toHaveBeenCalled();
		});

		it('a server answer that lands while the device is still answering wins', async () => {
			device.query.mockReturnValue(new Promise(() => {}));
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			expect(device.query).toHaveBeenCalled();
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
		});

		it('a device that fails and then a server that fails shows the server’s failure', async () => {
			device.query.mockRejectedValue(new Error('disk I/O error'));
			const { server, fail } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			const failure = new TypeError('Failed to fetch');
			const caught = result.catch((error) => error);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			fail(failure);
			expect(await caught).toBe(failure);
		});

		it('a device that fails after the hand-off falls back to the server’s answer', async () => {
			device.query.mockRejectedValue(new Error('disk I/O error'));
			const { server, answer } = hanging();
			const result = readSearch('search', '1', { q: 'a' }, server);
			await vi.advanceTimersByTimeAsync(SLOW_MS);
			answer(ON_SERVER);
			expect(await result).toEqual(ON_SERVER);
		});
	});
});
