import { page as view } from 'vitest/browser';
import { expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const state = vi.hoisted(() => ({
	page: {
		route: { id: '/albums' },
		url: new URL('http://localhost/albums'),
		data: { me: null as unknown, library: null, tagTypes: [] }
	}
}));
vi.mock('$app/state', () => state);
vi.mock('$app/paths', () => ({
	base: '',
	assets: '',
	asset: (file: string) => file,
	resolve: (path: string) => path,
	resolveRoute: (path: string) => path,
	match: async () => null
}));

const { default: Sidebar } = await import('./Sidebar.svelte');

it.each([
	['owner', true],
	['admin', false],
	['user', false]
])('shows the Audit log link to the %s: %s', async (role, shown) => {
	state.page.data.me = { id: '1', username: 'sam', displayName: 'Sam', role };
	render(Sidebar);
	await expect.element(view.getByRole('link', { name: 'Albums' })).toBeVisible();
	const link = view.getByRole('link', { name: 'Audit log' });
	if (shown) {
		await expect.element(link).toHaveAttribute('href', '/admin/audit');
	} else {
		expect(link.elements()).toHaveLength(0);
	}
});
