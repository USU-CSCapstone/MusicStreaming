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
import { realtime } from '$lib/realtime.svelte';
import Layout from './+layout.svelte';

const children = createRawSnippet(() => ({ render: () => '<p>page</p>' }));

describe('the app layout', () => {
	it('deletes the device copy of the catalog where nobody is signed in', async () => {
		route.id = '/login';
		await render(Layout, {
			data: { me: null, library: null, tagTypes: [] },
			children
		} as never);

		expect(catalog.signOut).toHaveBeenCalled();
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
			realtime.offline = true;
			await vi.advanceTimersByTimeAsync(10_000);
			realtime.offline = false;
			await vi.advanceTimersByTimeAsync(30_000);
			expect(catalog.prepareIndex).not.toHaveBeenCalled();

			realtime.offline = true;
			await vi.advanceTimersByTimeAsync(29_000);
			expect(catalog.prepareIndex).not.toHaveBeenCalled();
			await vi.advanceTimersByTimeAsync(1_000);
			expect(catalog.prepareIndex).toHaveBeenCalledWith('7');
		} finally {
			vi.useRealTimers();
			realtime.offline = false;
		}
	});
});
