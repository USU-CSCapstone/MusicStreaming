import { type Page } from '@playwright/test';
import type { Server } from './server';
import { createPlaylist, expect, test } from './test';

// Liking and hiding songs, and Liked Songs, against a real server (`requirements/playlists.md`
// §11, `requirements/conventions.md` §8).

const heart = (page: Page, title: string) => page.getByRole('button', { name: `Like “${title}”` });
const row = (page: Page, title: string) =>
	page.locator('main').getByRole('button', { name: title, exact: true });

// A like or a hide shows at once and reaches the server after, and a page loaded afresh, or a
// list the server builds, knows only what reached it. So each waits for the server to hold it.

/** Waits until the server holds the song titled `title` as liked, or not. */
async function likedOnServer(server: Server, title: string, liked = true) {
	const path = `/libraries/${server.ids.library}/tracks/${server.ids.tracks[title]}`;
	await expect.poll(async () => (await server.api(path)).liked, `${title} liked`).toBe(liked);
}

/** Waits until the server holds the song titled `title` as hidden, or not. */
async function hiddenOnServer(server: Server, title: string, hidden = true) {
	const path = `/libraries/${server.ids.library}/hidden-tracks?limit=100`;
	const id = server.ids.tracks[title];
	await expect
		.poll(async () => (await server.api(path)).items.some((t: { id: string }) => t.id === id), {
			message: `${title} hidden`
		})
		.toBe(hidden);
}

/** Opens Liked Songs from the playlists page. */
async function openLikedSongs(page: Page) {
	await page.goto('/playlists');
	await page.getByRole('link', { name: /^Liked Songs/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Liked Songs' })).toBeVisible();
}

test('a liked song tops Liked Songs, and unliked it leaves', async ({ page, server }) => {
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await heart(page, 'Signal Part 2').click();
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	// Liked Songs puts the latest like first, so Part 2 is in before Remix is liked.
	await likedOnServer(server, 'Signal Part 2');
	await heart(page, 'Signal Remix').click();
	await likedOnServer(server, 'Signal Remix');

	await openLikedSongs(page);
	await expect(page.getByText('2 songs · ')).toBeVisible();
	await expect(page.locator('.track .title')).toHaveText(['Signal Remix', 'Signal Part 2']);
	await heart(page, 'Signal Remix').click();
	await expect(row(page, 'Signal Remix')).toBeHidden();
	await likedOnServer(server, 'Signal Remix', false);

	// It stays liked as the album shows it again.
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	await expect(heart(page, 'Signal Remix')).toHaveAttribute('aria-pressed', 'false');
});

test('pressing a heart keeps the songs selected', async ({ page, server }) => {
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	await page.goto(`/playlists/${mix}`);
	const bar = page.getByRole('toolbar', { name: 'Selected songs' });
	await row(page, 'Signal Part 1').click();
	await row(page, 'Signal Part 2').click({ modifiers: ['Shift'] });
	await expect(bar).toContainText('2 songs selected');

	await heart(page, 'Signal Part 2').click();
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	await heart(page, 'Signal Remix').click();
	await expect(heart(page, 'Signal Remix')).toHaveAttribute('aria-pressed', 'true');
	await expect(bar).toContainText('2 songs selected');
	await expect(row(page, 'Signal Part 1')).toHaveAccessibleDescription('Selected');
	// So neither like lands after the next test has cleared them.
	await likedOnServer(server, 'Signal Part 2');
	await likedOnServer(server, 'Signal Remix');
});

test('a hidden song leaves every list until it is unhidden from Hidden Songs', async ({
	page,
	server
}) => {
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await hide(page, server, 'Signal Part 2');
	await expect(page.getByRole('status')).toHaveText(/Hid “Signal Part 2”/);

	await page.goto('/songs');
	await expect(row(page, 'Signal Part 1')).toBeVisible();
	await expect(row(page, 'Signal Part 2')).toBeHidden();

	await page.goto('/account');
	await page.getByRole('link', { name: /Hidden Songs · 1/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Hidden Songs' })).toBeVisible();
	// A list to unhide from, not to play.
	await expect(page.locator('main').getByRole('button', { name: 'Play' })).toBeHidden();
	await page.getByRole('button', { name: 'Unhide “Signal Part 2”' }).click();
	await expect(page.getByText(/^You haven't hidden any songs/)).toBeVisible();
	await hiddenOnServer(server, 'Signal Part 2', false);

	await page.goto('/songs');
	await expect(row(page, 'Signal Part 2')).toBeVisible();
});

test('an album added to Liked Songs leaves its hidden song hidden', async ({ page, server }) => {
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await hide(page, server, 'Signal Part 2');

	// The album as its page shows it, without the hidden song, which no like unhides.
	await page.getByRole('button', { name: 'Actions for “Signal”' }).click();
	const added = page.waitForResponse((r) => r.request().method() === 'PATCH');
	await page.getByRole('menuitem', { name: /^Liked Songs/ }).click();
	await expect(page.getByRole('status')).toHaveText(/Added “Signal” to “Liked Songs”/);
	expect((await added).status()).toBe(200);

	await openLikedSongs(page);
	await expect(row(page, 'Signal Part 1')).toBeVisible();
	await expect(row(page, 'Signal Part 2')).toBeHidden();
	await page.goto('/account/hidden');
	await expect(page.getByRole('button', { name: 'Unhide “Signal Part 2”' })).toBeVisible();
});

/** Hides `title` from its ⋯ menu on the page shown, and waits for the server to hold it. */
async function hide(page: Page, server: Server, title: string) {
	await page.getByRole('button', { name: `Actions for ${title}` }).click();
	await page.getByRole('menuitem', { name: 'Hide song' }).click();
	await expect(row(page, title)).toBeHidden();
	await hiddenOnServer(server, title);
}

/** Opens the queue panel from the player, and gives the panel and its Up Next's titles. */
async function openQueue(page: Page) {
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	const titles = (name: 'Up Next' | 'History') => () =>
		panel.getByRole('list', { name }).locator('[data-item] .title').allInnerTexts();
	return { panel, upNext: titles('Up Next'), history: titles('History') };
}

/** Starts the album by its Play button, and gives the track IDs of the list the server started. */
async function playAlbum(page: Page) {
	const started = page.waitForResponse(
		(r) => r.request().method() === 'POST' && new URL(r.url()).pathname === '/api/v1/me/queue'
	);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const { list } = await (await started).json();
	return list as string;
}

/** The track IDs of the queue list `hash`, as the server holds it. */
async function listed(server: Server, hash: string): Promise<string[]> {
	return (await server.api(`/me/queue/lists/${hash}`)).tracks;
}

test('an album played without its hidden song queues it not at all', async ({ page, server }) => {
	const { Signal } = server.ids.albums;
	const { tracks } = server.ids;
	await page.goto(`/albums/${Signal}`);
	await hide(page, server, 'Signal Part 2');
	const list = await playAlbum(page);
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	expect(await listed(server, list)).toEqual([tracks['Signal Part 1'], tracks['Signal Remix']]);
	const { upNext } = await openQueue(page);
	await expect.poll(upNext).toEqual(['Signal Remix']);
});

test('a song hidden once queued leaves the queue, is skipped, and stays out of it', async ({
	page,
	server
}) => {
	const { tracks } = server.ids;
	const edits: unknown[] = [];
	page.on('request', (r) => {
		if (r.method() === 'POST' && new URL(r.url()).pathname === '/api/v1/me/queue/edits') {
			edits.push(r.postDataJSON().edit);
		}
	});
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	const list = await playAlbum(page);
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	const first = await openQueue(page);
	await expect.poll(first.upNext).toEqual(['Signal Part 2', 'Signal Remix']);
	// The panel covers the album at this width.
	await first.panel.getByRole('button', { name: 'Close queue' }).click();

	await hide(page, server, 'Signal Part 2');
	const { panel, upNext } = await openQueue(page);
	await expect.poll(upNext).toEqual(['Signal Remix']);
	await expect(panel.getByRole('tab').first()).toHaveText(/Up Next 1/);
	// The queue keeps it, and the player passes over it.
	expect(await listed(server, list)).toEqual([
		tracks['Signal Part 1'],
		tracks['Signal Part 2'],
		tracks['Signal Remix']
	]);
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => edits).toEqual([{ type: 'next' }, { type: 'next' }]);

	// A page loaded afresh learns what is hidden from Hidden Songs: History shows only Part 1.
	await page.reload();
	const again = await openQueue(page);
	await again.panel.getByRole('tab', { name: /History/ }).click();
	await expect.poll(again.history).toEqual(['Signal Part 1']);
});
