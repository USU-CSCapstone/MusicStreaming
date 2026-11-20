import { createUser, expect, login, onDevice, test } from './test';

test('a user’s open app follows an admin taking their library away and giving it back, without a reload', async ({
	browser,
	server
}) => {
	const { library } = server.ids;
	const user = await createUser(server, [library]);
	const context = await browser.newContext({ baseURL: server.url });
	try {
		const page = await context.newPage();
		await login(page, server.url, user.username, user.password);
		await page.goto('/albums');
		await expect(page.getByText('Signal', { exact: true }).first()).toBeVisible();
		await expect.poll(() => onDevice(page, library), { timeout: 30_000 }).toBe(true);
		// Gone after a reload, so its presence at the end says the page was never loaded again.
		await page.evaluate(() => ((window as { stayed?: boolean }).stayed = true));
		// The realtime connection stays up throughout: a new one would say hello, and run the load again.
		let connections = 0;
		page.on('websocket', () => connections++);

		const libraries = `/admin/users/${user.id}/libraries`;
		await server.api(libraries, { method: 'PUT', body: { libraryIds: [] } });
		await expect(
			page.getByText("You don't have access to a library on this server yet.")
		).toBeVisible();
		await expect.poll(() => onDevice(page, library)).toBe(false);

		await server.api(libraries, { method: 'PUT', body: { libraryIds: [library] } });
		await expect(page.getByText('Signal', { exact: true }).first()).toBeVisible();
		expect(await page.evaluate(() => (window as { stayed?: boolean }).stayed)).toBe(true);
		expect(connections).toBe(0);
	} finally {
		await context.close();
	}
});
