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
