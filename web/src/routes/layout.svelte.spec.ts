import { describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { render } from 'vitest-browser-svelte';

const route = vi.hoisted(() => ({ id: '/login' as string | null }));
const catalog = vi.hoisted(() => ({
	signOut: vi.fn(),
	connect: vi.fn(),
	disconnect: vi.fn()
}));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog }));
vi.mock('$app/state', () => ({ page: { route } }));

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
});
