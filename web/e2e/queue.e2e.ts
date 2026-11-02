import { expect, test } from '@playwright/test';
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
