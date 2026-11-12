import { describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { render } from 'vitest-browser-svelte';

const route = vi.hoisted(() => ({ id: '/login' as string | null }));
const catalog = vi.hoisted(() => ({
	signOut: vi.fn(),
	connect: vi.fn(),
	disconnect: vi.fn(),
	setWorkingOffline: vi.fn(),
	prepareIndex: vi.fn()
}));
const queue = vi.hoisted(() => ({
	connect: vi.fn(),
	disconnect: vi.fn(),
	setWorkingOffline: vi.fn()
}));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog }));
// The real connection would reach for a server that is not there, and its own timers and failures
// would move the connectivity state under a test that runs the clock by hand.
vi.mock('$lib/realtime.svelte', async (original) => ({
	...(await original<typeof import('$lib/realtime.svelte')>()),
	realtime: { start: vi.fn(), stop: vi.fn(), on: vi.fn(() => () => {}) }
}));
const player = vi.hoisted(() => ({ libraryId: '', stopStreaming: vi.fn() }));
vi.mock('$lib/queue/client.svelte', () => ({ queue }));
vi.mock('$lib/player.svelte', () => ({ player }));
vi.mock('$app/state', () => ({
	page: {
		route,
		url: new URL('http://localhost/'),
		data: { me: null, library: null, tagTypes: [] }
	}
}));

import { connectivity } from '$lib/connectivity.svelte';
import { playback } from '$lib/playback.svelte';
import Layout from './+layout.svelte';

const children = createRawSnippet(() => ({ render: () => '<p>page</p>' }));

describe('the app layout', () => {
	it('leaves the catalog’s election, and keeps its files, where nobody is signed in', async () => {
		route.id = '/login';
		await render(Layout, {
			data: { me: null, library: null, tagTypes: [] },
			children
		} as never);

		expect(catalog.disconnect).toHaveBeenCalled();
		expect(catalog.signOut).not.toHaveBeenCalled();
		expect(catalog.connect).not.toHaveBeenCalled();
	});

	it('tells both workers when Work offline is turned on and off', async () => {
		route.id = '/login';
		await render(Layout, {
			data: { me: null, library: null, tagTypes: [] },
			children
		} as never);
		await vi.waitFor(() => expect(queue.setWorkingOffline).toHaveBeenLastCalledWith(false));

		expect(player.stopStreaming).not.toHaveBeenCalled();
		connectivity.setWorkOffline(true);
		await vi.waitFor(() => expect(queue.setWorkingOffline).toHaveBeenLastCalledWith(true));
		expect(player.stopStreaming).toHaveBeenCalled();
		expect(catalog.setWorkingOffline).toHaveBeenLastCalledWith(true);

		connectivity.setWorkOffline(false);
		await vi.waitFor(() => expect(catalog.setWorkingOffline).toHaveBeenLastCalledWith(false));
		expect(queue.setWorkingOffline).toHaveBeenLastCalledWith(false);
	});

	it('keeps the workers running when Work offline is turned on and off, though connecting reads it', async () => {
		route.id = '/';
		// As the queue's own `connect` does, to tell a new worker at once.
		queue.connect.mockImplementation(() => void connectivity.workOffline);
		try {
			await render(Layout, {
				data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
				children
			} as never);
			await vi.waitFor(() => expect(catalog.connect).toHaveBeenCalled());
			catalog.connect.mockClear();
			catalog.disconnect.mockClear();
			queue.connect.mockClear();
			queue.disconnect.mockClear();

			connectivity.setWorkOffline(true);
			await vi.waitFor(() => expect(catalog.setWorkingOffline).toHaveBeenLastCalledWith(true));
			connectivity.setWorkOffline(false);
			await vi.waitFor(() => expect(catalog.setWorkingOffline).toHaveBeenLastCalledWith(false));
			await new Promise((resolve) => setTimeout(resolve, 50));
			expect(catalog.disconnect).not.toHaveBeenCalled();
			expect(catalog.connect).not.toHaveBeenCalled();
			expect(queue.disconnect).not.toHaveBeenCalled();
			expect(queue.connect).not.toHaveBeenCalled();
		} finally {
			queue.connect.mockReset();
			connectivity.setWorkOffline(false);
			catalog.prepareIndex.mockClear();
		}
	});

	it('keeps the catalog connected when the load runs again for the same account', async () => {
		route.id = '/';
		const shown = await render(Layout, {
			data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
			children
		} as never);
		await vi.waitFor(() => expect(catalog.connect).toHaveBeenCalled());
		catalog.connect.mockClear();
		catalog.disconnect.mockClear();

		// As `invalidateAll` does: the same account, read again.
		await shown.rerender({
			data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] }
		} as never);
		await new Promise((resolve) => setTimeout(resolve, 50));
		expect(catalog.disconnect).not.toHaveBeenCalled();
		expect(catalog.connect).not.toHaveBeenCalled();
	});

	it('shows the playback kept for the account without starting its workers over', async () => {
		route.id = '/';
		localStorage.setItem(
			'jewelcase-playback:1',
			JSON.stringify({
				deviceId: '7',
				devices: [{ id: '7', name: 'Desk', type: 'desktop', connected: true }],
				player: { status: 'paused', activeDeviceId: '7', itemId: null, positionMs: 0 }
			})
		);
		catalog.connect.mockClear();
		const errors = vi.spyOn(console, 'error');
		try {
			await render(Layout, {
				data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
				children
			} as never);
			await vi.waitFor(() => expect(playback.devices).toHaveLength(1));
			// The state it shows changing, as realtime changes it, is no change of account.
			playback.state = { ...playback.state!, status: 'playing' };
			await new Promise((resolve) => setTimeout(resolve, 50));
			expect(catalog.connect).toHaveBeenCalledOnce();
			expect(errors).not.toHaveBeenCalled();
		} finally {
			localStorage.removeItem('jewelcase-playback:1');
			errors.mockRestore();
		}
	});

	it('has the search index of the library on screen built once the app is offline, and not before', async () => {
		route.id = '/';
		await render(Layout, {
			data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
			children
		} as never);
		await vi.waitFor(() => expect(catalog.connect).toHaveBeenCalled());
		expect(catalog.prepareIndex).not.toHaveBeenCalled();

		connectivity.setWorkOffline(true);
		await vi.waitFor(() => expect(catalog.prepareIndex).toHaveBeenCalledWith('7'));
		connectivity.setWorkOffline(false);
	});

	it('has it built only once a lost connection stays lost, not for a moment’s drop such as a server restart', async () => {
		route.id = '/';
		await render(Layout, {
			data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
			children
		} as never);
		catalog.prepareIndex.mockClear();
		vi.useFakeTimers();
		try {
			connectivity.lose();
			await vi.advanceTimersByTimeAsync(10_000);
			connectivity.reach();
			await vi.advanceTimersByTimeAsync(30_000);
			expect(catalog.prepareIndex).not.toHaveBeenCalled();

			connectivity.lose();
			await vi.advanceTimersByTimeAsync(29_000);
			expect(catalog.prepareIndex).not.toHaveBeenCalled();
			await vi.advanceTimersByTimeAsync(1_000);
			expect(catalog.prepareIndex).toHaveBeenCalledWith('7');
		} finally {
			vi.useRealTimers();
			connectivity.reach();
		}
	});

	it('has it built at once when the browser loses its network, though the connection was already lost', async () => {
		route.id = '/';
		await render(Layout, {
			data: { me: { id: '1' }, library: { id: '7' }, tagTypes: [] },
			children
		} as never);
		catalog.prepareIndex.mockClear();
		const network = vi.spyOn(navigator, 'onLine', 'get');
		vi.useFakeTimers();
		try {
			connectivity.lose();
			await vi.advanceTimersByTimeAsync(10_000);
			network.mockReturnValue(false);
			window.dispatchEvent(new Event('offline'));
			await vi.advanceTimersByTimeAsync(0);
			expect(catalog.prepareIndex).toHaveBeenCalledWith('7');
		} finally {
			vi.useRealTimers();
			network.mockRestore();
			window.dispatchEvent(new Event('online'));
			connectivity.reach();
		}
	});
});
