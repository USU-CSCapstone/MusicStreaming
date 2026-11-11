import { page } from 'vitest/browser';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const fake = vi.hoisted(() => ({
	calls: [] as string[],
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
vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: { signOut: async () => void fake.calls.push('wipe') }
}));
vi.mock('$lib/last-known', async (original) => ({
	...(await original<object>()),
	setLogoutPending: (on: boolean) => fake.calls.push(`marker ${on}`)
}));
// The server is never heard from, so the page stays where it is for the test to look at.
vi.mock('$lib/logout', () => ({
	finishLogout: () => {
		fake.calls.push('told');
		return new Promise(() => {});
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
	// The wait for the server is never over, so the page does not go to the login page.
	vi.useFakeTimers({ toFake: ['setTimeout'] });
});
afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});

it('wipes the device before it tells the server, so a server out of reach cannot keep the session on it', async () => {
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	// Realtime stops first, so its sign-out cannot race the login redirect; the marker is down
	// before the wipe, in case the page closes part-way.
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it.each([
	[true, 'The browser has agreed to keep it.'],
	[false, 'The browser has not agreed to keep it, and may clear it.']
])('says whether the browser granted persistent storage (%s)', async (granted, text) => {
	vi.spyOn(navigator.storage, 'persisted').mockResolvedValue(granted);
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByText(text)).toBeVisible();
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
