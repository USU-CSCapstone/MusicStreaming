import { createPlaylist, expect, go, onDevice, test } from './test';

// Playlists are made, edited and deleted the same way with Work offline on as off: each is a
// change the device makes at once and the server hears of when the device is online again, with
// no error shown (`requirements/offline.md` §6, §8).

test('playlists made, renamed, added to and deleted offline reach the server once back online', async ({
	page,
	server
}) => {
	const { library, albums } = server.ids;
	await createPlaylist(server, 'Old One', []);
	await createPlaylist(server, 'Old Two', []);
	const mine = async () =>
		(
			(await server.api(`/libraries/${library}/playlists?limit=1000`)).items as {
				title: string;
				trackCount: number;
			}[]
		)
			.filter((p) => p.title !== 'Liked Songs')
			.map((p) => `${p.title} (${p.trackCount})`)
			.sort();

	await page.goto('/playlists');
	await expect(page.getByRole('link', { name: /Old Two/ })).toBeVisible();
	await expect.poll(() => onDevice(page, library), { timeout: 30_000 }).toBe(true);
	// What the toasts say, kept, as they come and go.
	await page.evaluate(() => {
		const seen: string[] = [];
		(window as unknown as { toasts: string[] }).toasts = seen;
		new MutationObserver(() => {
			for (const el of document.querySelectorAll('[role="status"]'))
				if (el.textContent) seen.push(el.textContent);
		}).observe(document.body, { subtree: true, childList: true, characterData: true });
	});
	const requests: string[] = [];
	page.context().on('request', (request) => {
		if (request.url().includes('/api/v1/')) requests.push(request.url());
	});

	await go(page, '/account');
	await page.getByRole('switch', { name: 'Work offline' }).check();
	await go(page, '/playlists');
	requests.length = 0;

	// A new playlist opens at its own address at once.
	await page.getByRole('button', { name: 'New playlist' }).click();
	const dialog = page.getByRole('dialog', { name: 'New playlist' });
	await dialog.getByRole('textbox', { name: 'Title' }).fill('Offline Mix');
	await dialog.getByRole('button', { name: 'Create' }).click();
	await expect(page).toHaveURL(/\/playlists\/[1-9]\d*$/);
	await expect(page.getByRole('heading', { name: 'Offline Mix' })).toBeVisible();
	await expect(page.getByText('This playlist is empty.')).toBeVisible();
	const mixPath = new URL(page.url()).pathname;

	// A song is added to it from a menu.
	await go(page, `/albums/${albums.Signal}`);
	await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
	await page.getByRole('menuitem', { name: /Offline Mix/ }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Added' })).toHaveText(
		/Added “Signal Part 2” to “Offline Mix”/
	);

	// And it is renamed.
	await go(page, mixPath);
	await expect(page.locator('.track .title')).toHaveText(['Signal Part 2']);
	await page.getByRole('button', { name: 'Edit' }).click();
	const edit = page.getByRole('dialog', { name: 'Edit details' });
	await edit.getByRole('textbox', { name: 'Title' }).fill('Offline Mix 2');
	await edit.getByRole('button', { name: 'Save' }).click();
	await expect(edit).toBeHidden();
	await expect(page.getByRole('heading', { name: 'Offline Mix 2' })).toBeVisible();

	// One delete is undone, another is not.
	await go(page, '/playlists');
	await page.getByRole('link', { name: /Old Two/ }).click();
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await expect(page).toHaveURL(/\/playlists$/);
	await expect(page.getByRole('link', { name: /Old Two/ })).toBeHidden();
	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.getByRole('link', { name: /Old Two/ })).toBeVisible();
	await page.getByRole('link', { name: /Old One/ }).click();
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await expect(page.getByRole('link', { name: /Old One/ })).toBeHidden();
	await page.getByRole('button', { name: 'Dismiss' }).click();
	await expect(page.getByText('3 playlists', { exact: true })).toBeVisible();

	// Nothing went to the server, which still has what it had.
	expect(requests).toEqual([]);
	expect(await mine()).toEqual(['Old One (0)', 'Old Two (0)']);

	await go(page, '/account');
	await page.getByRole('switch', { name: 'Work offline' }).uncheck();
	await expect.poll(mine, { timeout: 30_000 }).toEqual(['Offline Mix 2 (1)', 'Old Two (0)']);

	// No error showed, at any point.
	const toasts = await page.evaluate(() => (window as unknown as { toasts: string[] }).toasts);
	expect(toasts.filter((text) => /Couldn’t|Work offline is on/.test(text))).toEqual([]);
});
