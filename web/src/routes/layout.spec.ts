import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getServerInfo: vi.fn(async () => ({ setupRequired: false })),
	getMe: vi.fn(),
	listLibraries: vi.fn(),
	listTagTypes: vi.fn()
}));
vi.mock('$lib/api/client', () => api);
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
const catalog = vi.hoisted(() => ({ signOut: vi.fn(async () => {}) }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog }));
const kept = vi.hoisted(() => ({
	owner: null as string | null,
	pending: false,
	claim: vi.fn(),
	remember: vi.fn()
}));
vi.mock('$lib/last-known', () => ({
	owner: () => kept.owner,
	claim: kept.claim,
	remember: kept.remember,
	logoutPending: () => kept.pending
}));
const logout = vi.hoisted(() => ({ finishLogout: vi.fn(async () => {}) }));
vi.mock('$lib/logout', () => logout);

import { load } from './+layout';

/** Runs the layout's load for the page at `route`. */
const loadAt = (route: string) =>
	load({ fetch, route: { id: route }, untrack: (f: () => unknown) => f() } as never) as Promise<{
		me: unknown;
	}>;

beforeEach(() => {
	kept.owner = null;
	kept.pending = false;
	for (const mock of [api.getMe, api.listLibraries, api.listTagTypes]) mock.mockReset();
	api.listLibraries.mockResolvedValue({ items: [] });
	catalog.signOut.mockClear();
	kept.claim.mockClear();
	kept.remember.mockClear();
	logout.finishLogout.mockClear();
});

describe('the app layout load', () => {
	it('sends a signed-in visit to the login page home, so it never wipes a valid session', async () => {
		api.getMe.mockResolvedValue({ id: '1' });
		await expect(loadAt('/login')).rejects.toMatchObject({ status: 307, location: '/' });
	});

	it('shows the login page where there is no session', async () => {
		api.getMe.mockRejectedValue(new Error('unauthenticated'));
		await expect(loadAt('/login')).resolves.toMatchObject({ me: null });
	});

	it('shows only the login page while a logout is waiting to be told, and never asks who is signed in', async () => {
		kept.pending = true;
		await expect(loadAt('/login')).resolves.toMatchObject({ me: null });
		await expect(loadAt('/albums')).rejects.toMatchObject({ status: 307, location: '/login' });
		expect(api.getMe).not.toHaveBeenCalled();
		expect(logout.finishLogout).toHaveBeenCalledTimes(2);
		expect(catalog.signOut).toHaveBeenCalled();
	});

	it('wipes what another account left before this one connects', async () => {
		kept.owner = '1';
		api.getMe.mockResolvedValue({ id: '2' });
		const order: string[] = [];
		catalog.signOut.mockImplementation(async () => void order.push('wiped'));
		api.listLibraries.mockImplementation(async () => (order.push('libraries'), { items: [] }));

		await loadAt('/');
		// The first answer may be the last account's; the one that is used comes after the wipe.
		expect(order).toEqual(['libraries', 'wiped', 'libraries']);
		expect(kept.remember).toHaveBeenCalledWith({ me: { id: '2' } });
		expect(kept.claim).toHaveBeenCalledWith('2');
	});

	it('keeps what the same account left, and claims a device nobody owned', async () => {
		kept.owner = '1';
		api.getMe.mockResolvedValue({ id: '1' });
		await loadAt('/');
		expect(catalog.signOut).not.toHaveBeenCalled();
		expect(kept.claim).not.toHaveBeenCalled();
		// The usual start asks for the libraries once, beside who is signed in.
		expect(api.listLibraries).toHaveBeenCalledTimes(1);

		kept.owner = null;
		await loadAt('/');
		expect(catalog.signOut).not.toHaveBeenCalled();
		expect(kept.claim).toHaveBeenCalledWith('1');
	});
});
