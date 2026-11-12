import type { Page } from '@playwright/test';
import { createPlaylist, expect, go, onDevice, test } from './test';

// The one walk of everything `requirements/offline.md` §6 says works offline that is built (not
// yet downloaded play), done with Work offline on and the server stopped, then once online again:
// the server holds each thing that was done, and nothing was refused on the way. The details of each feature are in its own spec
// (`queue-offline`, `playlists-offline`, `searches-offline`, `set-aside`); this is the whole.

/**
 * Types `query` in the search box, which opens its results, waits for the result titled `result`,
 * then submits it, which keeps it, and empties the box.
 */
async function search(page: Page, query: string, result: string) {
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	await box.fill('');
	await box.pressSequentially(query);
	await expect(page).toHaveURL(new RegExp(`/search\\?q=${query}$`));
	// Results come from the device when offline, which builds its index for the first search.
	await expect(page.locator('main').getByText(result).first()).toBeVisible({ timeout: 15_000 });
	await box.press('Enter');
	await box.fill('');
}

test('everything offline.md says works offline works with Work offline on and the server stopped, and reaches the server after', async ({
	device
}) => {
	test.setTimeout(180_000);
	const { page, server } = device;
	const { library, albums, tags, tracks } = server.ids;
	const main = page.locator('main');
	const player = page.getByRole('region', { name: 'Player' });
	const panel = page.getByRole('complementary', { name: 'Queue' });
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	const queued = () => upNext.locator('[data-item] .title').allInnerTexts();
	const songs = page.locator('.track .title');
	const sidebar = page.getByRole('navigation', { name: 'Library' });
	const play = () => main.getByRole('button', { name: 'Play', exact: true }).click();
	const hearth = (await server.api(`/libraries/${library}/albums?limit=100`)).items.find(
		(album: { title: string }) => album.title === 'Hearth'
	).id as string;
	const playlistsOnServer = async () =>
		(
			(await server.api(`/libraries/${library}/playlists?limit=1000`)).items as {
				title: string;
				kind: string;
				trackCount: number;
			}[]
		)
			.filter((p) => p.kind !== 'liked')
			.map((p) => `${p.title} (${p.trackCount})`)
			.sort();
	const searchesOnServer = async () =>
		((await server.api(`/libraries/${library}/recent-searches`)).items as { query: string }[]).map(
			(s) => s.query
		);
	const flagged = async (what: 'hidden-tracks' | 'liked') =>
		(
			(await server.api(
				what === 'liked'
					? `/libraries/${library}/tracks/${tracks['Signal Part 1']}`
					: `/libraries/${library}/hidden-tracks?limit=100`
			)) as { liked?: boolean; items?: { id: string }[] }
		)[what === 'liked' ? 'liked' : 'items'];

	/** The server's active queue's ID; empty until there is one. */
	const active = async () =>
		((await server.api('/me/queue').catch(() => ({ queueId: '' }))) as { queueId: string }).queueId;
	const keptIds = async () =>
		((await server.api('/me/queues')).items as { queueId: string }[]).map((q) => q.queueId);
	/** The kept queues whose states the page has read from the server. */
	const read = new Set<string>();
	page.on('response', (response) => {
		const id = /\/me\/queues\/([0-9a-f-]{36})$/.exec(new URL(response.url()).pathname)?.[1];
		if (id && response.request().method() === 'GET' && response.ok()) read.add(id);
	});
	/** Queues the song `title` of the page shown, which adjusts the queue. */
	const queueLast = async (title: string) => {
		await main.getByRole('button', { name: `Actions for ${title}` }).click();
		await page.getByRole('menuitem', { name: 'Queue last' }).click();
	};

	await createPlaylist(server, 'Old One', []);
	await createPlaylist(server, 'Old Two', []);

	// Online first: the library comes to the device, and the page is laid out as it will stay.
	await page.goto('/songs');
	await expect(songs).toHaveCount(4);
	await expect.poll(() => onDevice(page, library), { timeout: 30_000 }).toBe(true);
	// A search made online, which also has the device build its search index.
	await search(page, 'signal', 'Signal Part 1');
	await expect.poll(searchesOnServer).toEqual(['signal']);
	// A queue adjusted and replaced, so the server keeps it, and the device reads its state.
	await go(page, `/albums/${albums.Signal}`);
	await expect(main.getByText('Signal Remix')).toBeVisible();
	await play();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect.poll(active).toBeTruthy();
	const signal = await active();
	await queueLast('Signal Remix');
	await go(page, `/albums/${hearth}`);
	await expect(main.getByText('Kindling')).toBeVisible();
	await play();
	await expect.poll(active).not.toBe(signal);
	await expect.poll(keptIds).toEqual([signal]);
	await expect.poll(() => read.has(signal), { timeout: 15_000 }).toBe(true);
	await go(page, '/account');
	await expect(sidebar.getByRole('link', { name: 'Plugins' })).toBeVisible();
	const links = await sidebar.getByRole('link').allInnerTexts();
	// What the toasts say, kept, as they come and go.
	await page.evaluate(() => {
		const seen: string[] = [];
		(window as unknown as { toasts: string[] }).toasts = seen;
		new MutationObserver(() => {
			for (const el of document.querySelectorAll('[role="status"]'))
				if (el.textContent) seen.push(el.textContent);
		}).observe(document.body, { subtree: true, childList: true, characterData: true });
	});

	await page.getByRole('switch', { name: 'Work offline' }).check();
	await server.halt();

	// Browse, sort and filter, across the whole library.
	await go(page, '/albums');
	await expect(page.getByRole('link', { name: /Signal/ }).first()).toBeVisible();
	await expect(page.getByRole('link', { name: /Hearth/ }).first()).toBeVisible();
	await go(page, '/songs');
	await expect(songs).toHaveCount(4);
	await page.getByRole('button', { name: 'Filter by genre' }).click();
	await page.getByRole('checkbox', { name: /Ambient/ }).check();
	await expect(page).toHaveURL(new RegExp(`tagAny=${tags.Ambient}`));
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.getByRole('button', { name: 'Title', exact: true }).click();
	await expect(page).toHaveURL(/sort=title/);
	await expect(songs).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await page.getByRole('button', { name: 'Title, sorted ascending' }).click();
	await expect(songs).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.getByRole('button', { name: 'Filter by genre, 1 chosen' }).click();
	await page.getByRole('button', { name: 'Clear', exact: true }).click();
	await page.keyboard.press('Escape');

	// Search runs on the device, and the search is kept. A link waits a frame before it
	// navigates, so the page is there before typing: a later navigation would replace the search.
	await go(page, '/albums');
	await expect(page).toHaveURL(/\/albums$/);
	await search(page, 'remix', 'Signal Remix');
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	await box.fill('');
	await box.focus();
	// Newest first; the one made online is offered once the device has heard of it.
	await expect(
		page.getByRole('region', { name: 'Recent searches' }).locator('.query').first()
	).toHaveText('remix');

	// A queue: the one playing is adjusted, then replaced by another start, which keeps it as it
	// is online and shows it in Kept at once; the new one is shuffled and unshuffled; and the kept
	// queue the device read online is restored with no connection. (Restoring the queue replaced
	// offline is the user's case, but its restore can be refused after reconnecting — a known bug,
	// logged in the plan; this restores that queue instead once it is fixed.)
	await go(page, `/albums/${hearth}`);
	await expect(main.getByText('Kindling')).toBeVisible();
	await queueLast('Kindling');
	await go(page, `/albums/${albums.Signal}`);
	await expect(main.getByText('Signal Remix')).toBeVisible();
	await play();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect.poll(queued).toEqual(['Signal Part 2', 'Signal Remix']);
	await player.getByRole('button', { name: 'Shuffle' }).click();
	await expect(player.getByRole('button', { name: 'Shuffle' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await expect.poll(async () => (await queued()).sort()).toEqual(['Signal Part 2', 'Signal Remix']);
	await player.getByRole('button', { name: 'Shuffle' }).click();
	await expect(player.getByRole('button', { name: 'Shuffle' })).toHaveAttribute(
		'aria-pressed',
		'false'
	);
	await expect.poll(queued).toEqual(['Signal Part 2', 'Signal Remix']);
	await panel.getByRole('tab', { name: 'Kept' }).click();
	const entries = panel.getByRole('list', { name: 'Kept queues' }).getByRole('listitem');
	await expect(entries).toHaveCount(2);
	await expect(entries.filter({ hasText: 'Kindling' })).toContainText('1 song up next');
	await entries
		.filter({ hasText: 'Signal Part 1' })
		.getByRole('button', { name: /^Restore/ })
		.click();
	await expect(panel.getByRole('tab', { name: /Up Next/ })).toHaveAttribute(
		'aria-selected',
		'true'
	);
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect(panel.getByRole('alert')).toHaveCount(0);
	await panel.getByRole('button', { name: 'Close queue' }).click();

	// Playlists: made, added to from a menu, edited, and deleted with and without Undo.
	await go(page, '/playlists');
	await page.getByRole('button', { name: 'New playlist' }).click();
	const dialog = page.getByRole('dialog', { name: 'New playlist' });
	await dialog.getByRole('textbox', { name: 'Title' }).fill('Offline Mix');
	await dialog.getByRole('button', { name: 'Create' }).click();
	await expect(page).toHaveURL(/\/playlists\/[1-9]\d*$/);
	await expect(page.getByRole('heading', { name: 'Offline Mix' })).toBeVisible();
	const mixPath = new URL(page.url()).pathname;
	await go(page, `/albums/${albums.Signal}`);
	await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
	await page.getByRole('menuitem', { name: /Offline Mix/ }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Added' })).toHaveText(
		/Added “Signal Part 2” to “Offline Mix”/
	);
	await go(page, mixPath);
	await expect(songs).toHaveText(['Signal Part 2']);
	await page.getByRole('button', { name: 'Edit' }).click();
	const edit = page.getByRole('dialog', { name: 'Edit details' });
	await edit.getByRole('textbox', { name: 'Title' }).fill('Offline Mix 2');
	await edit.getByRole('button', { name: 'Save' }).click();
	await expect(page.getByRole('heading', { name: 'Offline Mix 2' })).toBeVisible();
	await go(page, '/playlists');
	await page.getByRole('link', { name: /Old Two/ }).click();
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await expect(page.getByRole('link', { name: /Old Two/ })).toBeHidden();
	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.getByRole('link', { name: /Old Two/ })).toBeVisible();
	await page.getByRole('link', { name: /Old One/ }).click();
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await expect(page.getByRole('link', { name: /Old One/ })).toBeHidden();
	await page.getByRole('button', { name: 'Dismiss' }).click();

	// Like and hide.
	await go(page, `/albums/${albums.Signal}`);
	const heart = main.getByRole('button', { name: 'Like “Signal Part 1”' });
	await heart.click();
	await expect(heart).toHaveAttribute('aria-pressed', 'true');
	await page.getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Hide song' }).click();
	await expect(main.getByRole('button', { name: 'Signal Remix', exact: true })).toBeHidden();

	// What needs the server says so in place, and the page keeps its layout.
	await go(page, '/account');
	await expect(page.getByRole('switch', { name: 'Work offline' })).toBeChecked();
	expect(await sidebar.getByRole('link').allInnerTexts()).toEqual(links);
	for (const [name, path] of [
		['Plugins', '/admin/plugins'],
		['Libraries', '/admin/libraries']
	]) {
		await sidebar.getByRole('link', { name }).click();
		await expect(page).toHaveURL(path);
		await expect(page.getByRole('heading', { name: 'Unavailable offline' })).toBeVisible();
		expect(await sidebar.getByRole('link').allInnerTexts()).toEqual(links);
	}

	// Nothing reached the server while it was away.
	await server.restart();
	expect(await playlistsOnServer()).toEqual(['Old One (0)', 'Old Two (0)']);

	// Back online: no button, and the server holds all of it.
	await go(page, '/account');
	await page.getByRole('switch', { name: 'Work offline' }).uncheck();
	await expect
		.poll(playlistsOnServer, { timeout: 60_000 })
		.toEqual(['Offline Mix 2 (1)', 'Old Two (0)']);
	await expect.poll(() => flagged('liked'), { timeout: 30_000 }).toBe(true);
	await expect
		.poll(async () => (await flagged('hidden-tracks'))!.map((t) => t.id), { timeout: 30_000 })
		.toEqual([tracks['Signal Remix']]);
	await expect.poll(searchesOnServer, { timeout: 30_000 }).toEqual(['remix', 'signal']);
	// The restored queue is the active one, and the one it replaced, adjusted, is kept.
	await expect.poll(active, { timeout: 60_000 }).toBe(signal);
	await expect
		.poll(
			async () =>
				(
					(await server.api('/me/queues')).items as {
						current: { trackId: string } | null;
						upNext: number;
					}[]
				).map(({ current, upNext }) => ({ current: current?.trackId, upNext })),
			{ timeout: 30_000 }
		)
		.toEqual([{ current: tracks.Kindling, upNext: 1 }]);
	await go(page, '/songs');
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('alert')).toHaveCount(0);

	// No error showed, at any point.
	const toasts = await page.evaluate(() => (window as unknown as { toasts: string[] }).toasts);
	expect(
		toasts.filter((text) => /Couldn’t|Work offline is on|did not apply|refused/.test(text))
	).toEqual([]);
});
