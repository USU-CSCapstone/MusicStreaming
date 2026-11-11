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
	catalog: {
		signOut: async () => void fake.calls.push('wipe')
	}
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
const { connectivity } = await import('$lib/connectivity.svelte');
const { sender } = await import('$lib/sync/sender');
const { memoryEnv } = await import('$lib/sync/store');

beforeEach(() => {
	fake.calls.length = 0;
	fake.role = 'owner';
	localStorage.clear();
	// The wait for the server is never over, so the page does not go to the login page.
	vi.useFakeTimers({ toFake: ['setTimeout'] });
});
afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
	sender.stop();
	connectivity.setWorkOffline(false);
});

it('wipes the device before it tells the server, so a server out of reach cannot keep the session on it', async () => {
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	// Realtime stops first, so its sign-out cannot race the login redirect; the marker is down
	// before the wipe, in case the page closes part-way.
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('asks before deleting what has not reached the server, and deletes only on yes', async () => {
	connectivity.setWorkOffline(true);
	sender.start('1', { env: memoryEnv() });
	sender.enqueue('play', { playId: 'p1' } as never);
	sender.enqueue('mark', { library: '1', track: '5', liked: true, what: 'like' });
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect
		.element(page.getByText("2 plays and changes haven't reached the server. Log out anyway?"))
		.toBeVisible();
	expect(fake.calls).toEqual([]);

	// No is no: nothing is deleted, and the button is back.
	await page.getByRole('button', { name: 'Cancel' }).click();
	expect(fake.calls).toEqual([]);
	await expect.element(page.getByRole('button', { name: 'Log out', exact: true })).toBeVisible();
	expect(await sender.pending()).toBe(2);

	await page.getByRole('button', { name: 'Log out', exact: true }).click();
	await page.getByRole('button', { name: 'Log out anyway' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('logs out without asking when everything has been sent', async () => {
	sender.start('1', { env: memoryEnv() });
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('tries to send what is waiting before it asks, and logs out without asking once it is sent', async () => {
	const posted: string[] = [];
	let answers = 0;
	/** A server that is busy for its first answer, then accepts the plays it is sent. */
	const fetcher = (async (_input: RequestInfo | URL, init: RequestInit = {}) => {
		// The first try is refused as the server being busy, so the play waits for its time.
		if (answers++ === 0) return new Response('{}', { status: 503 });
		for (const item of JSON.parse(String(init.body)).items) posted.push(item.playId);
		return new Response(null, { status: 204 });
	}) as typeof fetch;
	sender.start('1', { env: memoryEnv(), fetch: fetcher });
	sender.enqueue('play', { playId: 'p1' } as never);
	await vi.waitFor(() => expect(answers).toBe(1));
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
	expect(posted).toEqual(['p1']);
	await expect.element(page.getByText("haven't reached the server")).not.toBeInTheDocument();
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
