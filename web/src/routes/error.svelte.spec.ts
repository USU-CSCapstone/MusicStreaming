import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const state = vi.hoisted(() => ({
	page: { status: 500, error: undefined, data: {} } as {
		status: number;
		error?: { message: string; code?: string };
		data: object;
	}
}));
vi.mock('$app/state', () => ({ page: state.page }));

import ErrorPage from './+error.svelte';

describe('the error page', () => {
	it("tells a non-admin an admin page is for admins, from the server's forbidden code alone", async () => {
		// The gate's 403 carries a code and no detail, so the message is the status text.
		state.page.status = 403;
		state.page.error = { message: 'Forbidden', code: 'forbidden' };
		const screen = await render(ErrorPage);
		await expect.element(screen.getByRole('heading', { name: 'Admins only' })).toBeVisible();
		await expect
			.element(screen.getByText('This page is for the admins of this server.'))
			.toBeVisible();
	});

	it('still reports any other error as before', async () => {
		state.page.status = 500;
		state.page.error = { message: 'Boom' };
		const screen = await render(ErrorPage);
		await expect
			.element(screen.getByRole('heading', { name: 'Something went wrong' }))
			.toBeVisible();
		await expect.element(screen.getByText('Boom')).toBeVisible();
	});
});
