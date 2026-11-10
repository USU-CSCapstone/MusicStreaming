import { page } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const fake = vi.hoisted(() => ({
	calls: [] as string[],
	logout: vi.fn<() => Promise<void>>()
}));

vi.mock('$lib/realtime.svelte', () => ({
	realtime: {
		connected: false,
		offline: false,
		on: () => () => {},
		start: () => fake.calls.push('start'),
		stop: () => fake.calls.push('stop')
	}
}));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<object>()),
	logout: () => {
		fake.calls.push('logout');
		return fake.logout();
	}
}));
vi.mock('$app/state', () => ({
	page: {
		route: { id: '/account' },
		data: { me: { displayName: 'Owner', username: 'owner', role: 'owner' }, tagTypes: [] }
	}
}));

const { default: Account } = await import('./+page.svelte');

beforeEach(() => {
	fake.calls.length = 0;
});

it('stops the realtime connection before logging out, so its sign-out cannot race the login redirect', async () => {
	fake.logout.mockReturnValue(new Promise(() => {}));
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	expect(fake.calls).toEqual(['stop', 'logout']);
});

it('restarts the realtime connection when logging out fails', async () => {
	fake.logout.mockRejectedValue(new Error('Server down'));
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent('Server down');
	expect(fake.calls).toEqual(['stop', 'logout', 'start']);
});
