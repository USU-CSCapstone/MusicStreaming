import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { redirect } from '@sveltejs/kit';

const api = vi.hoisted(() => ({ logout: vi.fn() }));
vi.mock('$lib/api/client', () => api);

import { finishLogout } from './logout';

let marker: string | null;
/** What waits for the browser to say it is online. */
let waiting: (() => void)[];

/** The error SvelteKit's `redirect` throws. */
const redirected = () => {
	try {
		redirect(307, '/login');
	} catch (error) {
		return error;
	}
};

beforeEach(() => {
	marker = '1';
	api.logout.mockReset();
	vi.stubGlobal('localStorage', {
		getItem: () => marker,
		setItem: (_: string, value: string) => (marker = value),
		removeItem: () => (marker = null)
	});
	waiting = [];
	vi.stubGlobal('addEventListener', (_: string, listener: () => void) => waiting.push(listener));
});

afterEach(() => vi.unstubAllGlobals());

/** Says the browser is online, as its event does. */
const online = () => waiting.splice(0).forEach((listener) => listener());

describe('a logout the server has not heard of', () => {
	it('is told to the server, and the marker goes once it answers', async () => {
		api.logout.mockResolvedValue(undefined);
		await finishLogout(fetch);
		expect(api.logout).toHaveBeenCalledOnce();
		expect(marker).toBeNull();
	});

	it('is as good as told where the session was over already', async () => {
		api.logout.mockRejectedValue(redirected());
		await finishLogout(fetch);
		expect(marker).toBeNull();
	});

	it('stays marked while the server cannot be reached, and is told the first time the browser is online', async () => {
		api.logout.mockRejectedValue(new TypeError('network'));
		await finishLogout(fetch);
		expect(marker).toBe('1');

		api.logout.mockResolvedValue(undefined);
		online();
		await vi.waitFor(() => expect(marker).toBeNull());
		expect(api.logout).toHaveBeenCalledTimes(2);
	});

	it('is not sent again once a login has taken the marker down', async () => {
		marker = null;
		await finishLogout(fetch);
		expect(api.logout).not.toHaveBeenCalled();
	});
});
