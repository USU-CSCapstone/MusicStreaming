import { page } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const fake = vi.hoisted(() => ({
	calls: [] as string[],
	logout: vi.fn<() => Promise<void>>(),
	role: 'owner' as 'owner' | 'admin' | 'user'
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
		get data() {
			return { me: { displayName: 'Owner', username: 'owner', role: fake.role }, tagTypes: [] };
		}
	}
}));

const { default: Account } = await import('./+page.svelte');

beforeEach(() => {
	fake.calls.length = 0;
	fake.role = 'owner';
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

it('offers administration to an admin, and not to a user', async () => {
	fake.role = 'admin';
	const admin = page.getByRole('region', { name: 'Administration' });
	const shown = render(Account, { data: { connectable: [] } } as never);
	await expect.element(admin.getByRole('link', { name: 'Libraries' })).toBeVisible();
	await expect.element(admin.getByRole('link', { name: 'Plugins' })).toBeVisible();
	shown.unmount();

	fake.role = 'user';
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByRole('heading', { name: 'Devices' })).toBeVisible();
	expect(admin.elements()).toHaveLength(0);
});
