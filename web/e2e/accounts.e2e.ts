import { createUser, expect, login, onDevice, test } from './test';

test('the owner takes a user’s library away, suspends them, and restores them, and their open app follows', async ({
	page,
	browser,
	server
}) => {
	const { library } = server.ids;
	const user = await createUser(server, [library]);
	const context = await browser.newContext({ baseURL: server.url });
	try {
		const theirs = await context.newPage();
		await login(theirs, server.url, user.username, user.password);
		await theirs.goto('/albums');
		await expect(theirs.getByText('Signal', { exact: true }).first()).toBeVisible();
		await expect.poll(() => onDevice(theirs, library), { timeout: 30_000 }).toBe(true);

		await page.goto('/admin/accounts');
		await page.getByRole('searchbox', { name: 'Search accounts' }).fill(user.username);
		await page.getByRole('button', { name: 'Search' }).click();
		const row = page.getByRole('listitem').filter({ hasText: user.username });
		await expect(row).toHaveCount(1);

		// Their only library, revoked: their open app says so, with no reload.
		await row.getByRole('button', { name: 'Libraries' }).click();
		await row
			.getByRole('group', { name: `Libraries ${user.username} can reach` })
			.getByRole('checkbox')
			.uncheck();
		await row.getByRole('button', { name: 'Save libraries' }).click();
		await expect(
			theirs.getByText("You don't have access to a library on this server yet.")
		).toBeVisible();

		// Suspended: thrown out to the login page, and the right password no longer signs in.
		await row.getByRole('button', { name: 'Suspend', exact: true }).click();
		await page.getByRole('dialog').getByRole('button', { name: 'Suspend' }).click();
		await expect(row.getByText('Suspended', { exact: true })).toBeVisible();
		await expect(theirs).toHaveURL(/\/login(\?|$)/);
		const refused = await theirs.request.post(`${server.url}/api/v1/auth/login`, {
			headers: { 'X-Jewelcase-Client': 'web' },
			data: {
				username: user.username,
				password: user.password,
				device: { name: 'Again', type: 'desktop' }
			}
		});
		expect(refused.ok()).toBe(false);

		// Restored: they sign in again.
		await row.getByRole('button', { name: 'Restore' }).click();
		await expect(row.getByText('Suspended', { exact: true })).toHaveCount(0);
		await login(theirs, server.url, user.username, user.password);
		await theirs.goto('/albums');
		await expect(
			theirs.getByText("You don't have access to a library on this server yet.")
		).toBeVisible();
	} finally {
		await context.close();
	}
});

test('a user signs their other browser out from the Account page, and it ends up at the login page', async ({
	browser,
	server
}) => {
	const user = await createUser(server, [server.ids.library]);
	const first = await browser.newContext({ baseURL: server.url });
	const second = await browser.newContext({ baseURL: server.url });
	try {
		const here = await first.newPage();
		await login(here, server.url, user.username, user.password, {
			name: 'First browser',
			type: 'desktop'
		});
		const there = await second.newPage();
		await login(there, server.url, user.username, user.password, {
			name: 'Second browser',
			type: 'desktop'
		});
		await there.goto('/albums');
		await expect(there.getByText('Signal', { exact: true }).first()).toBeVisible();

		await here.goto('/account');
		const devices = here.getByRole('list', { name: 'Devices' });
		await expect(devices.getByRole('listitem').filter({ hasText: 'Second browser' })).toBeVisible();
		await expect(here.getByRole('button', { name: 'Sign out First browser' })).toHaveCount(0);
		await here.getByRole('button', { name: 'Sign out Second browser' }).click();

		await expect(there).toHaveURL(/\/login(\?|$)/);
		await expect(devices.getByText('Second browser')).toHaveCount(0);
		await expect(devices.getByText('First browser')).toBeVisible();
	} finally {
		await first.close();
		await second.close();
	}
});
