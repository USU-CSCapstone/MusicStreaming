import { expect, test, type Page } from '@playwright/test';
import { queueOf, serveLibrary } from './fixtures';

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

test('a tag-filtered view starts from the songs it shows, not a recipe', async ({ page }) => {
	await serveLibrary(page);
	const queue = queueOf(page);
	await page.goto('/songs?tagAny=61');
	await expect(page.locator('.track .title')).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect.poll(() => queue.starts.length).toBeGreaterThan(0);
	// A recipe can't name tags yet, so the start sends the filtered songs themselves.
	for (const start of queue.starts) {
		expect(start).toMatchObject({ source: { tracks: ['42', '41'] }, start: 1 });
		expect(start.source).not.toHaveProperty('recipe');
	}
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
