import { describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getServerInfo: vi.fn(async () => ({ setupRequired: false })),
	getMe: vi.fn(),
	listLibraries: vi.fn(),
	listTagTypes: vi.fn()
}));
vi.mock('$lib/api/client', () => api);
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import { load } from './+layout';

/** Runs the layout's load for the page at `route`. */
const loadAt = (route: string) =>
	load({ fetch, route: { id: route }, untrack: (f: () => unknown) => f() } as never) as Promise<{
		me: unknown;
	}>;

describe('the app layout load', () => {
	it('sends a signed-in visit to the login page home, so it never wipes a valid session', async () => {
		api.getMe.mockResolvedValue({ id: '1' });
		await expect(loadAt('/login')).rejects.toMatchObject({ status: 307, location: '/' });
	});

	it('shows the login page where there is no session', async () => {
		api.getMe.mockRejectedValue(new Error('unauthenticated'));
		await expect(loadAt('/login')).resolves.toMatchObject({ me: null });
	});
});
