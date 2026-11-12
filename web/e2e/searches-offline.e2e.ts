import type { Page } from '@playwright/test';
import { expect, go, onDevice, test } from './test';

// Recent searches are kept, offered, removed and cleared the same way with Work offline on as off:
// each is a change the device makes at once and the server hears of when the device is online
// again (`requirements/search.md` §6, `requirements/offline.md` §6).

/** Searches for `query` in the box and submits it, which keeps it, then empties the box. */
async function search(page: Page, query: string) {
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	await box.fill('');
	await box.pressSequentially(query);
	await expect(page).toHaveURL(new RegExp(`/search\\?q=${query}$`));
	await box.press('Enter');
	await box.fill('');
}

test('searches kept, removed and cleared offline reach the server once back online', async ({
	page,
	server
}) => {
	const { library } = server.ids;
	const onServer = async () =>
		((await server.api(`/libraries/${library}/recent-searches`)).items as { query: string }[]).map(
			(s) => s.query
		);
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	const recent = page.getByRole('region', { name: 'Recent searches' });
	const offered = recent.locator('.query');

	await page.goto('/albums');
	await expect.poll(() => onDevice(page, library), { timeout: 30_000 }).toBe(true);
	await search(page, 'signal');
	await expect.poll(onServer).toEqual(['signal']);

	await go(page, '/account');
	await page.getByRole('switch', { name: 'Work offline' }).check();
	// Counted from here: an online read begun before may still ask the server.
	const requests: string[] = [];
	page.context().on('request', (request) => {
		if (request.url().includes('/recent-searches')) requests.push(request.url());
	});
	await go(page, '/albums');
	await expect(page).toHaveURL(/\/albums$/);

	// Kept and offered at once, newest first, with the one kept online.
	for (const query of ['remix', 'bonus', 'aurora']) await search(page, query);
	await box.focus();
	await expect(offered).toHaveText(['aurora', 'bonus', 'remix', 'signal']);

	// One kept offline and the one kept online are removed.
	for (const query of ['bonus', 'signal']) {
		await recent.getByRole('button', { name: `Remove “${query}” from recent searches` }).click();
	}
	await expect(offered).toHaveText(['aurora', 'remix']);
	// The device has let them go: what it offers after another search is read from it.
	await search(page, 'help');
	await expect(offered).toHaveText(['help', 'aurora', 'remix']);

	// Nothing went to the server, which still has what it had.
	expect(requests).toEqual([]);
	expect(await onServer()).toEqual(['signal']);

	await go(page, '/account');
	await page.getByRole('switch', { name: 'Work offline' }).uncheck();
	await expect.poll(onServer, { timeout: 30_000 }).toEqual(['help', 'aurora', 'remix']);

	// Cleared offline, they are gone at once, and from the server once back online.
	await page.getByRole('switch', { name: 'Work offline' }).check();
	await box.focus();
	await expect(offered).toHaveText(['help', 'aurora', 'remix']);
	await recent.getByRole('button', { name: 'Clear all' }).click();
	await expect(recent).toHaveCount(0);
	await search(page, 'mix');
	await expect(offered).toHaveText(['mix']);
	expect(await onServer()).toEqual(['help', 'aurora', 'remix']);
	await page.getByRole('switch', { name: 'Work offline' }).uncheck();
	await expect.poll(onServer, { timeout: 30_000 }).toEqual(['mix']);
});
