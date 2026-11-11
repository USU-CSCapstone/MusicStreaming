import { expect, test, type Page } from '@playwright/test';
import { longMix, queueOf, serveLibrary, serveQueueEdits, signalMix } from './fixtures';

test('plays an album from the queue', async ({ page }) => {
	const { plays } = await serveLibrary(page);
	const queue = queueOf(page);
	// The server answers the start only once the test lets it: the album plays before that.
	let answer!: () => void;
	queue.holdStarts = new Promise((resolve) => (answer = resolve));

	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect(player.getByRole('button', { name: 'Pause' })).toBeVisible();
	await expect.poll(() => plays.length).toBeGreaterThan(0);
	expect(queue.starts, 'not answered yet').toEqual([]);

	answer();
	await expect.poll(() => queue.starts.length).toBe(1);
	// The album's recipe, with the row clicked and how many rows the view has, not its tracks.
	expect(queue.starts[0]).toMatchObject({
		context: { type: 'album', id: '31' },
		source: {
			libraryId: '11',
			recipe: { type: 'album', albumId: '31', sort: 'album', order: 'asc' }
		},
		start: 0,
		startTrackId: '41'
	});
	expect(queue.starts[0].source).not.toHaveProperty('tracks');

	// Next moves the queue on, and plays what it moves to.
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => queue.edits).toEqual([{ type: 'next' }, { type: 'next' }]);

	// Every play records the album it was played from.
	await expect
		.poll(() => plays.find((p) => p.trackId === '43'))
		.toMatchObject({ context: { type: 'album', id: '31' }, origin: 'context' });
});

test('a tag-filtered view starts by a recipe carrying its filter', async ({ page }) => {
	await serveLibrary(page);
	const queue = queueOf(page);
	await page.goto('/songs?tagAny=61');
	await expect(page.locator('.track .title')).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect.poll(() => queue.starts.length).toBe(1);
	// The start names the filter, not the songs, and the server's resolve is the rows shown.
	for (const start of queue.starts) {
		expect(start).toMatchObject({
			source: { recipe: { type: 'library', filters: { tagAny: ['61'] } } },
			startTrackId: '41',
			context: { type: 'filter', filter: { tagAny: ['61'] } }
		});
		expect(start.source).not.toHaveProperty('tracks');
	}
	expect([...queue.lists.values()].at(-1)).toEqual(['42', '41']);
});

/** Starts the album on its first track, with the queue panel open, and answers the start. */
async function openUpNext(page: Page) {
	const { plays } = await serveLibrary(page);
	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await player.getByRole('button', { name: 'Queue' }).click();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	return { panel, plays, list: panel.getByRole('list', { name: 'Up Next' }) };
}

/**
 * Waits, in the queue panel, until a page just loaded has fetched the queue again, then closes
 * the panel. Queueing before the page has a queue only says to start something first.
 */
async function queueLoaded(page: Page) {
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	await expect(panel.getByRole('tab').first()).toHaveText(/Up Next [1-9]/);
	await panel.getByRole('button', { name: 'Close queue' }).click();
}

test('reorders and removes in Up Next', async ({ page }) => {
	const queue = queueOf(page);
	const { panel, list } = await openUpNext(page);
	const titles = () => list.locator('[data-item] .title').allInnerTexts();
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	await expect(panel.getByText('Nothing queued.')).toBeVisible();

	// Dragging the grip of the first row below the last moves it to the end of the context.
	const grip = list.locator('[data-item]').first().locator('.grip');
	const [from, last] = [
		await grip.boundingBox(),
		await list.locator('[data-item]').last().boundingBox()
	];
	await page.mouse.move(from!.x + 5, from!.y + 5);
	await page.mouse.down();
	await page.mouse.move(from!.x + 5, from!.y + 40, { steps: 4 });
	await page.mouse.move(from!.x + 5, last!.y + last!.height - 4, { steps: 4 });
	await page.mouse.up();
	await expect.poll(titles).toEqual(['Signal Remix', 'Signal Part 2']);
	expect(queue.edits.at(-1)).toMatchObject({ type: 'move', before: { type: 'contextEnd' } });

	// Alt+↑ from the keyboard moves the last row up, and up again across the Manual Queue's end.
	await list.getByRole('button', { name: 'Signal Part 2', exact: true }).focus();
	await page.keyboard.press('Alt+ArrowUp');
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	await page.keyboard.press('Alt+ArrowUp');
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	expect(queue.edits.at(-1)).toMatchObject({ type: 'move', before: { type: 'manualEnd' } });
	await expect(panel.getByText('Nothing queued.')).toBeHidden();

	// One press removes a row from anywhere, the Manual Queue's included.
	await panel.getByRole('button', { name: 'Remove Signal Part 2' }).click();
	await expect.poll(titles).toEqual(['Signal Remix']);
	expect(queue.edits.at(-1)).toMatchObject({ type: 'remove' });
	await expect(panel.getByText('Nothing queued.')).toBeVisible();

	// Clicking a row plays it, the rows before it going to History.
	await list.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Remix')
	).toBeVisible();
	expect(queue.edits.at(-1)).toMatchObject({ type: 'jump' });
});

test('reaches Shuffle and Repeat from the panel at phone width', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	const queue = queueOf(page);
	const { panel } = await openUpNext(page);
	// The player bar has no room for them.
	await expect(
		page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Shuffle' })
	).toBeHidden();
	await panel.getByRole('button', { name: 'Shuffle' }).click();
	await expect(panel.getByRole('button', { name: 'Shuffle' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await panel.getByRole('button', { name: 'Repeat off' }).click();
	await expect(panel.getByRole('button', { name: 'Repeat all' })).toBeVisible();
	await expect
		.poll(() => queue.edits.map((e) => e.type))
		.toEqual(expect.arrayContaining(['shuffle', 'repeat']));
	expect(queue.edits.at(-1)).toMatchObject({ type: 'repeat', mode: 'all' });
});

/** Plays the album, skips `nexts` tracks, then opens the queue panel and its History tab. */
async function openHistory(page: Page, nexts: number) {
	const { panel, list } = await openUpNext(page);
	const player = page.getByRole('region', { name: 'Player' });
	for (let i = 0; i < nexts; i++) await player.getByRole('button', { name: 'Next' }).click();
	await panel.getByRole('tab', { name: /History/ }).click();
	return { panel, player, list, history: panel.getByRole('list', { name: 'History' }) };
}

test('rewinds from History', async ({ page }) => {
	const queue = queueOf(page);
	const { panel, player, history } = await openHistory(page, 2);
	await expect(player.getByText('Signal Remix')).toBeVisible();
	const titles = () => history.locator('[data-item] .title').allInnerTexts();
	// Newest first.
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Part 1']);

	// Choosing the older play goes back to it, as Previous twice would.
	await history.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({ type: 'rewind', play: { play: 0 }, key: expect.any(String) });
	await expect(panel.getByText('Nothing has played yet.')).toBeVisible();
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	await expect
		.poll(() => upNext.locator('[data-item] .title').allInnerTexts())
		.toEqual(['Signal Part 2', 'Signal Remix']);
});

test('re-queues a played track', async ({ page }) => {
	const queue = queueOf(page);
	const { panel, player, history } = await openHistory(page, 1);
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await expect(history.getByRole('button', { name: 'Signal Part 1', exact: true })).toBeVisible();

	await panel.getByRole('button', { name: 'Queue Signal Part 1 next' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({ type: 'requeue', play: { play: 0 }, key: expect.any(String) });
	await expect(panel.getByText('Nothing has played yet.')).toBeVisible();

	// It is at the front of Up Next, in the Manual Queue, and the song playing is undisturbed.
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect
		.poll(() => upNext.locator('[data-item] .title').allInnerTexts())
		.toEqual(['Signal Part 1', 'Signal Remix']);
	await expect(player.getByText('Signal Part 2')).toBeVisible();
});

/** Plays the album from its page, with the realtime connection telling the page what the queue applies. */
async function playAlbum(page: Page) {
	await serveLibrary(page);
	await serveQueueEdits(page);
	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	/** The titles of the rows in Up Next, in order. */
	const manual = () =>
		panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts();
	return { player, panel, manual };
}

test('queues an album next', async ({ page }) => {
	const queue = queueOf(page);
	const { player, panel } = await playAlbum(page);

	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue next' }).click();
	await expect(page.getByRole('status').getByText('“Signal” queued next')).toBeVisible();
	// The album's recipe goes, not its tracks, at the Manual Queue's front.
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: {
				libraryId: '11',
				recipe: { type: 'album', albumId: '31', sort: 'album', order: 'asc' }
			},
			before: { type: 'manualStart' }
		});
	expect(queue.edits.at(-1)).not.toHaveProperty('source.tracks');

	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('heading', { name: /Queued by you\s*3/ })).toBeVisible();
	await expect
		.poll(() =>
			panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts()
		)
		.toEqual(['Signal Part 1', 'Signal Part 2', 'Signal Remix', 'Signal Part 2', 'Signal Remix']);
});

test('queues a song last', async ({ page }) => {
	const queue = queueOf(page);
	const { player, panel, manual } = await playAlbum(page);

	const row = page.locator('main').getByRole('button', { name: 'Actions for Signal Remix' });
	await row.click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect(page.getByRole('status').getByText('“Signal Remix” queued last')).toBeVisible();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { libraryId: '11', tracks: ['43'] },
			before: { type: 'manualEnd' }
		});
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect.poll(manual).toEqual(['Signal Remix', 'Signal Part 2', 'Signal Remix']);
});

test('drops a song into Up Next where it lands', async ({ page }) => {
	const queue = queueOf(page);
	const { player, panel, manual } = await playAlbum(page);
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Remix']);

	// Dragged from the album's row onto the gap above the context's last row.
	const row = page.locator('main').locator('li.track', { hasText: 'Signal Part 1' }).first();
	const last = panel.locator('[data-item]').last();
	const box = (await last.boundingBox())!;
	await row.dragTo(panel.getByRole('list', { name: 'Up Next' }), {
		targetPosition: {
			x: 80,
			y: box.y - (await panel.getByRole('list', { name: 'Up Next' }).boundingBox())!.y + 2
		}
	});
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { tracks: ['41'] },
			before: { type: 'before', item: expect.stringMatching(/\.2$/) }
		});
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
});

test('adds to a chosen point of Up Next from the keyboard', async ({ page }) => {
	const queue = queueOf(page);
	const { panel, manual } = await playAlbum(page);
	await page.locator('main').getByRole('button', { name: 'Actions for Signal Part 1' }).focus();
	await page.keyboard.press('Enter');
	await page.getByRole('menuitem', { name: 'Add to Up Next…' }).click();

	// The panel opens to choose where: Escape would give it up.
	const choose = panel.getByRole('group', { name: 'Choose where to add' });
	await expect(choose).toBeVisible();
	await expect(choose.getByRole('button', { name: 'Add to the end' })).toBeFocused();
	await panel
		.getByRole('button', { name: 'Add “Signal Part 1” before Signal Remix' })
		.press('Enter');
	await expect(choose).toBeHidden();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { tracks: ['41'] },
			before: { type: 'before', item: expect.stringMatching(/\.2$/) }
		});
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	// Focus stays on the row it was added before.
	await expect(panel.getByRole('button', { name: 'Signal Remix', exact: true })).toBeFocused();
});

test('adds an album to a chosen point of Up Next from the keyboard', async ({ page }) => {
	const queue = queueOf(page);
	const { panel } = await playAlbum(page);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).focus();
	await page.keyboard.press('Enter');
	await page.getByRole('menuitem', { name: 'Add to Up Next…' }).click();

	const choose = panel.getByRole('group', { name: 'Choose where to add' });
	await choose.getByRole('button', { name: 'Add to the end' }).press('Enter');
	await expect(choose).toBeHidden();
	// The recipe is posted to the worker, which cannot take what the page's state wraps.
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', albumId: '31' } }
		});
});

test('queues a sorted playlist by a recipe that resolves to the rows shown', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await serveQueueEdits(page);
	await page.goto(`/playlists/${signalMix.id}?sort=title&order=desc`);
	const shown = ['Signal Remix', 'Signal Part 2', 'Signal Part 1'];
	await expect(page.locator('main .track .title')).toHaveText(shown);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Remix')
	).toBeVisible();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0]).toMatchObject({
		context: { type: 'playlist', id: signalMix.id },
		source: { recipe: { type: 'playlist', playlistId: signalMix.id, sort: 'name', order: 'desc' } }
	});
	expect(queue.starts[0].source).not.toHaveProperty('tracks');
	expect([...queue.lists.values()][0]).toEqual(['43', '42', '41']);

	await page.locator('main').getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'playlist', sort: 'name', order: 'desc' } }
		});
	expect([...queue.lists.values()].at(-1)).toEqual(['43', '42', '41']);
});

test('a sorted, tag-filtered playlist starts by a recipe resolving to the rows shown', async ({
	page
}) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	// The start is not answered yet, so Up Next is what the device made from the rows shown.
	let answer!: () => void;
	queue.holdStarts = new Promise((resolve) => (answer = resolve));
	// Added: Part 1 (Jan 1), Remix (Jan 2), Part 2 (Jan 3); Calm leaves Part 2 out.
	await page.goto(`/playlists/${signalMix.id}?sort=added&order=desc&tagAny=63`);
	await expect(page.locator('main .track .title')).toHaveText(['Signal Remix', 'Signal Part 1']);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	// The two rows shown, so one is up next and none is behind, as the start's total says.
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	const tabs = page.getByRole('complementary', { name: 'Queue' }).getByRole('tab');
	await expect(tabs).toHaveText(['Up Next 1', 'History 0', 'Kept']);
	answer();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0]).toMatchObject({
		source: {
			recipe: {
				type: 'playlist',
				playlistId: signalMix.id,
				filters: { tagAny: ['63'] },
				sort: 'addedToPlaylist',
				order: 'desc'
			}
		},
		context: { type: 'playlist', id: signalMix.id }
	});
	expect([...queue.lists.values()][0]).toEqual(['43', '41']);
});

test('an unsorted, unfiltered playlist starts by a recipe of its own order', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	await expect(page.locator('main .track .title')).toHaveCount(3);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0].source.recipe).toEqual({ type: 'playlist', playlistId: signalMix.id });
	expect(queue.starts[0]).toMatchObject({ start: 0, startTrackId: '41' });
	expect([...queue.lists.values()][0]).toEqual(['41', '42', '43']);
});

test('a filtered playlist sends its songs until a tag type is known, then a recipe', async ({
	page
}) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await serveQueueEdits(page);
	await page.goto(`/playlists/${signalMix.id}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);

	// Tag 70 is a genre no song carries, and its type is not read yet.
	const release = Promise.withResolvers<void>();
	await page.route('**/tags/70', async (route) => {
		await release.promise;
		await route.fulfill({
			json: {
				id: '70',
				type: 'genre',
				name: 'Ambient Folk',
				trackCount: 0,
				albumCount: 0,
				artistCount: 0
			}
		});
	});
	await page.goto(`/playlists/${signalMix.id}?tagAny=61,70`);
	await queueLoaded(page);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Nothing is shown to send: no recipe went for rows the server might read differently.
	await expect(page.getByRole('status')).toContainText('no songs in “Signal Mix”');
	expect(queue.edits).toEqual([]);

	release.resolve();
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'playlist', filters: { tagAny: ['61', '70'] } } }
		});
});

test('queues a tag-filtered view by a recipe carrying its filter', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page);
	await serveQueueEdits(page);
	// The album filtered to Calm: Part 2 is left out.
	await page.goto('/albums/31?tagAny=63');
	const shown = ['Signal Part 1', 'Signal Remix'];
	await expect(page.locator('main .track .title')).toHaveText(shown);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Part 1')
	).toBeVisible();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts.at(-1)).toMatchObject({
		source: { recipe: { type: 'album', albumId: '31', filters: { tagAny: ['63'] } } }
	});
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', albumId: '31', filters: { tagAny: ['63'] } } }
		});
	expect(queue.edits.at(-1)).not.toHaveProperty('source.tracks');
	// What the server resolves for it is exactly the songs shown.
	expect([...queue.lists.values()].at(-1)).toEqual(['41', '43']);

	// All the songs filtered to Ambient, the same way.
	await page.goto('/songs?tagAny=61');
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await queueLoaded(page);
	await page.locator('main').getByRole('button', { name: 'Actions for all songs' }).click();
	await page.getByRole('menuitem', { name: 'Queue next' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'library', filters: { tagAny: ['61'] } } }
		});
	expect([...queue.lists.values()].at(-1)).toEqual(['42', '41']);
});

test('a filtered album sends its songs until a tag type is known, then a recipe', async ({
	page
}) => {
	const queue = queueOf(page);
	await serveLibrary(page);
	await serveQueueEdits(page);
	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);

	// Tag 70 is a genre no song carries, and its type is not read yet.
	const release = Promise.withResolvers<void>();
	await page.route('**/tags/70', async (route) => {
		await release.promise;
		await route.fulfill({
			json: {
				id: '70',
				type: 'genre',
				name: 'Ambient Folk',
				trackCount: 0,
				albumCount: 0,
				artistCount: 0
			}
		});
	});
	await page.goto('/albums/31?tagAny=61,70');
	await queueLoaded(page);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Nothing is shown to send: no recipe went for rows the server might read differently.
	await expect(page.getByRole('status')).toContainText('no songs in “Signal”');
	expect(queue.edits).toEqual([]);

	release.resolve();
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', filters: { tagAny: ['61', '70'] } } }
		});
});

test('queues every song a filtered search finds', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page);
	await serveQueueEdits(page);
	await page.goto('/search?q=signal&tagAny=61');
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).click();
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Part 2')
	).toBeVisible();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts.at(-1)).toMatchObject({
		source: { recipe: { type: 'search', q: 'signal', filters: { tagAny: ['61'] } } }
	});
	await page.locator('main').getByRole('button', { name: 'Actions for all songs found' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => queue.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'search', q: 'signal', filters: { tagAny: ['61'] } } }
		});
	expect([...queue.lists.values()].at(-1)).toEqual(['41', '42']);
});

test('says to start something first when there is no queue to add to', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect(
		page.getByRole('status').getByText('Start something playing first, then queue more.')
	).toBeVisible();
	expect(queueOf(page).edits).toEqual([]);
});

test('a queued-into queue is kept and restores where it was', async ({ page }) => {
	const queue = queueOf(page);
	const { player, panel } = await playAlbum(page);
	// Paused, so the short fixture audio does not end and move the queue on.
	await player.getByRole('button', { name: 'Pause' }).click();
	// Queue a song, so the queue was adjusted, and say where playback was left.
	await page.locator('main').getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect.poll(() => queue.edits.at(-1)).toMatchObject({ type: 'add' });
	queue.positionMs = 1_000;
	const first = queue.queueId;

	// Starting another queue replaces it, and keeps it.
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(2);
	expect([...queue.kept.keys()]).toEqual([first]);

	await player.getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	const entry = panel.getByRole('list', { name: 'Kept queues' }).getByRole('listitem');
	await expect(entry).toHaveCount(1);
	await expect(entry).toContainText('Signal Part 1');
	await expect(entry).toContainText('Signal');
	await expect(entry).toContainText('3 songs up next');
	await expect(entry).toContainText('0 plays played');

	queue.reports.length = 0;
	await entry.getByRole('button', { name: /^Restore/ }).click();
	await expect.poll(() => queue.restores).toEqual([first]);
	// It is the queue shown, and the first thing reported is a start of its item, where it was left.
	await expect(panel.getByRole('tab', { name: /Up Next/ })).toHaveAttribute(
		'aria-selected',
		'true'
	);
	await expect
		.poll(() => queue.reports[0])
		.toMatchObject({ itemId: expect.stringMatching(/\.0$/), start: true, positionMs: 1_000 });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect(
		panel.getByRole('list', { name: 'Up Next' }).getByRole('button', { name: 'Signal Remix' })
	).toHaveCount(2);

	// The queue it replaced was never adjusted, so nothing is kept now.
	await panel.getByRole('tab', { name: 'Kept' }).click();
	await expect(panel.getByText(/kept here for 30 days/)).toBeVisible();
});

test('deletes a kept queue with one press', async ({ page }) => {
	const queue = queueOf(page);
	const { player, panel } = await playAlbum(page);
	await page.locator('main').getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// A start drops the edits not yet sent, so the queue is adjusted, and kept, only once it is.
	await expect.poll(() => queue.edits.at(-1)).toMatchObject({ type: 'add' });
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.kept.size).toBe(1);

	await player.getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	await panel.getByRole('button', { name: /^Delete/ }).click();
	await expect(panel.getByText(/kept here for 30 days/)).toBeVisible();
	expect(queue.kept.size).toBe(0);
});

test('restores a kept queue on a device that holds no playback, while its lists are slow', async ({
	page
}) => {
	const queue = queueOf(page);
	const { player, panel } = await playAlbum(page);
	await player.getByRole('button', { name: 'Pause' }).click();
	await page.locator('main').getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect.poll(() => queue.edits.at(-1)).toMatchObject({ type: 'add' });
	queue.positionMs = 1_000;
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.kept.size).toBe(1);

	// A fresh page holds no playback and no lists, and the server is slow to give them.
	queue.delayMs = 1_500;
	await page.reload();
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	queue.reports.length = 0;
	await panel.getByRole('button', { name: /^Restore/ }).click();
	await expect.poll(() => queue.restores.length).toBe(1);
	await expect
		.poll(() => queue.reports[0], { timeout: 10_000 })
		.toMatchObject({ itemId: expect.stringMatching(/\.0$/), start: true, positionMs: 1_000 });
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Part 1')
	).toBeVisible();
});

test('a filtered playlist queues as many pending rows as it shows', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await serveQueueEdits(page);
	await page.goto('/albums/31');
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);
	// The server does not answer the add yet.
	const release = Promise.withResolvers<void>();
	await page.route('**/api/v1/me/queue/edits', async (route) => {
		await release.promise;
		await route.fallback();
	});
	await page.goto(`/playlists/${signalMix.id}?tagAny=63`);
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 1', 'Signal Remix']);
	await queueLoaded(page);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	// Two rows, not the playlist's three.
	await expect(
		page
			.getByRole('complementary', { name: 'Queue' })
			.getByRole('heading', { name: /Queued by you\s*2/ })
	).toBeVisible();
	release.resolve();
});

test('a start from a clicked row of a playlist names the row and its track', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [longMix] });
	await page.goto(`/playlists/${longMix.id}`);
	// The second Signal Part 2 is row 4; the first is row 1.
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).nth(1).dblclick();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0]).toMatchObject({
		source: { recipe: { type: 'playlist', playlistId: longMix.id } },
		start: 4,
		startTrackId: '42'
	});
	// The server starts at that row, not the track's first place.
	expect(JSON.parse(queue.queue!.currentRow()!).item).toMatch(/\.4$/);
});

test('a playlist with edits not yet saved starts from the rows shown, then by recipe', async ({
	page
}) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	const release = Promise.withResolvers<void>();
	await page.route(`**/playlists/${signalMix.id}`, async (route) => {
		if (route.request().method() !== 'PATCH') return route.fallback();
		await release.promise;
		await route.fallback();
	});
	await page.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 1', 'Signal Part 2']);
	const play = page.locator('main').getByRole('button', { name: 'Play', exact: true });
	await play.click();
	// The server still has the removed song, so the songs shown go.
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0].source).toMatchObject({ tracks: ['41', '42'] });
	expect(queue.starts[0].source).not.toHaveProperty('recipe');

	const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
	release.resolve();
	await saved;
	await expect(async () => {
		await play.click();
		expect(queue.starts.at(-1)!.source).toHaveProperty('recipe.type', 'playlist');
	}).toPass();
});

test('a sorted playlist queued while an edit saves goes in the order shown', async ({ page }) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await serveQueueEdits(page);
	await page.goto(`/playlists/${signalMix.id}?sort=title&order=desc`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);
	const release = Promise.withResolvers<void>();
	await page.route(`**/playlists/${signalMix.id}`, async (route) => {
		if (route.request().method() !== 'PATCH') return route.fallback();
		await release.promise;
		await route.fallback();
	});
	await page.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await expect(page.locator('main .track .title')).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.locator('main').getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Not yet saved, so the songs go as shown, not in the playlist's own order.
	await expect
		.poll(() => queue.edits.findLast((edit) => edit.type === 'add'))
		.toMatchObject({ source: { tracks: ['42', '41'] } });
	release.resolve();
});

test('a playlist sorted by date added plays the rows shown once songs were put back', async ({
	page
}) => {
	const queue = queueOf(page);
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}?sort=added&order=asc`);
	const titles = page.locator('main .track .title');
	// Added together, so in track order.
	await expect(titles).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
	const saved = () => page.waitForResponse((r) => r.request().method() === 'PATCH');
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	let save = saved();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await save;
	save = saved();
	await page.getByRole('button', { name: 'Undo' }).click();
	await save;
	// The server dated the song put back as added now, the page as it was, so the rows shown go.
	await expect(titles).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => queue.starts.length).toBe(1);
	expect(queue.starts[0].source).toMatchObject({ tracks: ['41', '42', '43'] });
	expect(queue.starts[0].source).not.toHaveProperty('recipe');
});
