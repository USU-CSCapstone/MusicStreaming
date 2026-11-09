import type { Page } from '@playwright/test';
import { createPlaylist, expect, test } from './test';

// Sorting track lists (`requirements/conventions.md` §4.1). The tracks were added in the order
// Signal Part 1, Part 2, Remix, then Kindling, which is on another album.

const SIGNAL = ['Signal Part 1', 'Signal Part 2', 'Signal Remix'];

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

	test('sorts by a column, and plays as shown', async ({ page, server }) => {
		const id = await createPlaylist(server, 'Signal Mix', SIGNAL);
		await page.goto(`/playlists/${id}`);
		await expect(titles(page)).toHaveText(SIGNAL);

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

	test('moves nothing while sorted, but still removes the songs picked', async ({
		page,
		server
	}) => {
		const id = await createPlaylist(server, 'Signal Mix', SIGNAL);
		const sent = edits(page);
		await page.goto(`/playlists/${id}?sort=title&order=desc`);
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

	test('sorts by when each song was added to it', async ({ page, server }) => {
		const id = await createPlaylist(server, 'Signal Mix', SIGNAL);
		await page.goto(`/playlists/${id}`);
		// Signal Part 1 again, added now, after the rest.
		await page.getByRole('button', { name: 'Actions for Signal Part 1' }).click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		await page.getByRole('button', { name: 'Add anyway' }).click();
		await expect(titles(page)).toHaveText([...SIGNAL, 'Signal Part 1']);

		const ids = server.ids.tracks;
		// The songs added together follow the track ID, which the server picked at random.
		const byId = SIGNAL.toSorted((a, b) => (BigInt(ids[a]) < BigInt(ids[b]) ? -1 : 1));
		await heading(page, 'Date added').click();
		await expect(titles(page)).toHaveText([...byId, 'Signal Part 1']);
		await heading(page, 'Date added, sorted ascending').click();
		// Newest first, the ties reversed (design/general.md §3).
		await expect(titles(page)).toHaveText(['Signal Part 1', ...byId.toReversed()]);
	});
});

test('an album sorts as one run, and plays as shown', async ({ page, server }) => {
	await page.setViewportSize({ width: 1280, height: 700 });
	await page.goto(`/albums/${server.ids.albums.Signal}`);
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
	let searches = 0;
	page.on('request', (r) => {
		if (/\/api\/v1\/libraries\/[^/]+\/search\?/.test(r.url())) searches++;
	});
	const answered = page.waitForResponse((r) =>
		/\/api\/v1\/libraries\/[^/]+\/search\?/.test(r.url())
	);
	await page.goto('/search?q=signal');
	// The order they matched in is the server's to say.
	const found = (await (await answered).json()).sections.find(
		(section: { type: string }) => section.type === 'tracks'
	);
	const matched: string[] = found.items.map(
		(item: { track: { title: string } }) => item.track.title
	);
	expect(matched.toSorted()).toEqual(SIGNAL);
	await expect(titles(page)).toHaveText(matched);

	await heading(page, 'Title').click();
	await heading(page, 'Title, sorted ascending').click();
	await expect(page).toHaveURL(/\?q=signal&sort=title&order=desc$/);
	await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
	// Unsorted is the order they matched in.
	await heading(page, 'Title, sorted descending').click();
	await expect(titles(page)).toHaveText(matched);
	expect(searches).toBe(1);
});

test.describe('songs, which the server sorts', () => {
	test.beforeEach(async ({ page }) => {
		await page.setViewportSize({ width: 1280, height: 700 });
	});

	test('the library opens newest first, and a heading steps through its sorts', async ({
		page
	}) => {
		await page.goto('/songs');
		const newest = ['Kindling', 'Signal Remix', 'Signal Part 2', 'Signal Part 1'];
		await expect(titles(page)).toHaveText(newest);
		await expect(page.getByRole('button', { name: /sorted/ })).toHaveCount(0);

		await heading(page, 'Title').click();
		await expect(page).toHaveURL(/\?sort=title&order=asc$/);
		await expect(titles(page)).toHaveText(['Kindling', ...SIGNAL]);
		await heading(page, 'Title, sorted ascending').click();
		await expect(titles(page)).toHaveText([
			'Signal Remix',
			'Signal Part 2',
			'Signal Part 1',
			'Kindling'
		]);

		// A reload asks the server for the same order.
		await page.reload();
		await expect(titles(page)).toHaveText([
			'Signal Remix',
			'Signal Part 2',
			'Signal Part 1',
			'Kindling'
		]);

		await heading(page, 'Title, sorted descending').click();
		await expect(page).not.toHaveURL(/sort=/);
		await expect(titles(page)).toHaveText(newest);
	});

	test('sorts from a menu where the headings do not fit', async ({ page }) => {
		await page.setViewportSize({ width: 390, height: 700 });
		await page.goto('/songs');
		await expect(titles(page)).toHaveText([
			'Kindling',
			'Signal Remix',
			'Signal Part 2',
			'Signal Part 1'
		]);

		await page.getByRole('button', { name: 'Sort', exact: true }).click();
		await page.getByRole('menuitemradio', { name: 'Title' }).click();
		await expect(page).toHaveURL(/\?sort=title&order=asc$/);
		await expect(titles(page)).toHaveText(['Kindling', ...SIGNAL]);
		await expect(page.getByRole('button', { name: 'Sort, by title ascending' })).toBeFocused();
	});

	test("an artist's songs sort without loading the rest of the page again", async ({
		page,
		server
	}) => {
		const asked: string[] = [];
		page.on('request', (r) => {
			const path = new URL(r.url()).pathname;
			if (/\/api\/v1\/libraries\/[^/]+\/(artists|albums|tracks)/.test(path)) asked.push(path);
		});
		await page.goto(`/artists/${server.ids.artists['Aurora Lane']}`);
		const songs = page
			.locator('section')
			.filter({ has: page.getByRole('heading', { name: 'Songs' }) });
		// Unsorted, an artist's songs are in album order.
		await expect(titles(page)).toHaveText(SIGNAL);
		// The page's other requests are done before the count starts.
		await page.waitForLoadState('networkidle');
		asked.length = 0;

		await songs.getByRole('button', { name: 'Title', exact: true }).click();
		await songs.getByRole('button', { name: 'Title, sorted ascending' }).click();
		await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
		expect(asked.every((path) => path.endsWith('/tracks'))).toBe(true);
		expect(asked).toHaveLength(2);

		await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
		await expect(playing(page)).toHaveText('Signal Remix');

		// A link to the sorted songs opens sorted.
		await page.goto(`/artists/${server.ids.artists['Aurora Lane']}?sort=title&order=desc`);
		await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 2', 'Signal Part 1']);
		await expect(songs.getByRole('button', { name: 'Title, sorted descending' })).toBeVisible();
	});
});
