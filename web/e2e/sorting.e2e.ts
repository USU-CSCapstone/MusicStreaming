import { type Page, expect, test } from '@playwright/test';
import { album, serveLibrary, signalMix } from './fixtures';

// Sorting track lists (`requirements/conventions.md` §4.1).

const titles = (page: Page) => page.locator('.track .title');
const heading = (page: Page, name: string) => page.getByRole('button', { name, exact: true });
const playing = (page: Page) => page.locator('.track.current .title');

/** Every playlist edit the page sends. */
function edits(page: Page) {
	const sent: unknown[] = [];
	page.on('request', (r) => {
		if (r.method() === 'PATCH') sent.push(r.postDataJSON());
	});
	return sent;
}

test.describe('a playlist', () => {
	test.beforeEach(async ({ page }) => {
		await page.setViewportSize({ width: 1280, height: 700 });
	});

	test('sorts by a column, and plays as shown', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);

		await heading(page, 'Title').click();
		await heading(page, 'Title, sorted ascending').click();
		await expect(page).toHaveURL(/\?sort=title&order=desc$/);
		await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);

		await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
		await expect(playing(page)).toHaveText('Signal Remix');

		// Unsorted is the playlist's own order again.
		await heading(page, 'Title, sorted descending').click();
		await expect(page).not.toHaveURL(/sort=/);
		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
	});

	test('moves nothing while sorted, but still removes the songs picked', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		const sent = edits(page);
		await page.goto(`/playlists/${signalMix.id}?sort=title&order=desc`);
		await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);

		// Neither the keyboard nor a drag moves a song.
		await titles(page).nth(1).click();
		await page.keyboard.press('Alt+ArrowUp');
		await expect(page.getByText('The list is sorted, so songs can’t be moved')).toBeAttached();
		const from = (await page.locator('.track').nth(2).locator('.duration').boundingBox())!;
		await page.mouse.move(from.x, from.y);
		await page.mouse.down();
		await page.mouse.move(from.x, from.y - 120, { steps: 8 });
		await page.mouse.up();
		await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
		expect(sent).toEqual([]);

		// The first row shown is the last in the playlist, and that is the one removed.
		await page.keyboard.press('Escape');
		await titles(page).first().click();
		const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
		await page.getByRole('toolbar').getByRole('button', { name: 'Remove' }).click();
		expect((await saved).request().postDataJSON().ops).toEqual([{ op: 'remove', positions: [2] }]);
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Part 1']);

		// The sort is in the URL, so it survives a reload.
		await page.reload();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Part 1']);
		await expect(heading(page, 'Title, sorted descending')).toBeVisible();
	});

	test('sorts by when each song was added to it', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		// Signal Part 1 again, added now, after the rest.
		await page.getByRole('button', { name: 'Add Signal Part 1 to a playlist' }).click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		await page.getByRole('button', { name: 'Add anyway' }).click();
		const order = ['Signal Part 1', 'Signal Part 2', 'Signal Remix', 'Signal Part 1'];
		await expect(titles(page)).toHaveText(order);

		await heading(page, 'Date added').click();
		await expect(titles(page)).toHaveText(order);
		await heading(page, 'Date added, sorted ascending').click();
		// Newest first; the songs added together keep the playlist's order.
		await expect(titles(page)).toHaveText([
			'Signal Part 1',
			'Signal Part 1',
			'Signal Part 2',
			'Signal Remix'
		]);
	});
});

test('an album sorts as one run, and plays as shown', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 700 });
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await expect(page.getByText('Disc 2')).toBeVisible();
	// Every song shares the album, so it heads no Album or Year column.
	await expect(heading(page, 'Album')).toHaveCount(0);
	await expect(heading(page, 'Year')).toHaveCount(0);

	await heading(page, 'Title').click();
	await heading(page, 'Title, sorted ascending').click();
	await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
	await expect(page.getByText('Disc 2')).toHaveCount(0);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(playing(page)).toHaveText('Signal Remix');
});

test('search results sort without searching again', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 700 });
	await serveLibrary(page);
	let searches = 0;
	page.on('request', (r) => {
		if (/\/api\/v1\/libraries\/[^/]+\/search\?/.test(r.url())) searches++;
	});
	await page.goto('/search?q=signal');
	await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);

	await heading(page, 'Title').click();
	await heading(page, 'Title, sorted ascending').click();
	await expect(page).toHaveURL(/\?q=signal&sort=title&order=desc$/);
	await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
	// Unsorted is the order they matched in.
	await heading(page, 'Title, sorted descending').click();
	await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
	expect(searches).toBe(1);
});
