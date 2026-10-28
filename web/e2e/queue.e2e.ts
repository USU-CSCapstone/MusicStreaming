import { expect, type Locator, type Page, test } from '@playwright/test';
import { queueOf, serveLibrary, serveRealtime } from './fixtures';

// Track IDs by title, for building queues: Part 1, Part 2 and Remix.
const [P1, P2, R] = ['41', '42', '43'];
/** The height of a row of the panel's lists. */
const ROW = 56;

/**
 * The queue panel, opened from the player bar, with a queue of the fixture's songs. Looking up
 * tracks takes `lookupMs` more, as over a slow link.
 */
async function openQueue(page: Page, queue: Parameters<typeof serveRealtime>[1], lookupMs = 0) {
	await serveLibrary(page);
	if (lookupMs) {
		// After the fixture's routes, so it is asked first.
		await page.context().route('**/tracks?ids=**', async (route) => {
			await new Promise((resolve) => setTimeout(resolve, lookupMs));
			await route.fallback();
		});
	}
	await serveRealtime(page, queue);
	await page.goto('/albums');
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Queue' }).click();
	return page.getByRole('complementary', { name: 'Queue' });
}

/** What each row of the list says, in order: a heading's words, or an item's title. */
const rows = (list: Locator) =>
	list
		.locator('li.row')
		.evaluateAll((els) =>
			els.map((el) => el.querySelector('.title')?.textContent?.trim() ?? el.textContent!.trim())
		);

/** The queue items of the list's rows, in order. */
const items = (list: Locator) =>
	list
		.locator('li.row[data-item]')
		.evaluateAll((els) => els.map((el) => el.getAttribute('data-item')));

/** Drags the handle of `row` to `y` pixels below the top of the list. */
async function dragTo(page: Page, row: Locator, list: Locator, y: number) {
	const grip = (await row.locator('.grip').boundingBox())!;
	const top = (await list.locator('xpath=..').boundingBox())!.y;
	await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
	await page.mouse.down();
	await page.mouse.move(grip.x + 10, top + y, { steps: 8 });
	await page.mouse.up();
}

// One song queued by hand, then the context of four after what plays: Part 2, Remix, Part 1, Part 2.
const queued = { queued: [P1, P2, R, P1, P2], start: 0, manual: [R] };

test('Up Next shows what is queued by hand apart from the context, and History is empty', async ({
	page
}) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect
		.poll(() => rows(list))
		.toEqual([
			'Queued by you 1',
			'Signal Remix',
			'Up next 4',
			'Signal Part 2',
			'Signal Remix',
			'Signal Part 1',
			'Signal Part 2'
		]);
	await panel.getByRole('tab', { name: /History/ }).click();
	await expect(panel.getByText('Nothing has played yet.')).toBeVisible();
});

test('Alt+arrows move a song, across the end of the Manual Queue too', async ({ page }) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.getByRole('button', { name: 'Signal Part 1', exact: true })).toBeVisible();

	// Part 2, the context's first, steps down one place.
	await list.getByRole('button', { name: 'Signal Part 2', exact: true }).first().focus();
	await page.keyboard.press('Alt+ArrowDown');
	await expect
		.poll(() => rows(list))
		.toEqual([
			'Queued by you 1',
			'Signal Remix',
			'Up next 4',
			'Signal Remix',
			'Signal Part 2',
			'Signal Part 1',
			'Signal Part 2'
		]);
	// Focus stays with the song it moved, which moves up twice: back, then into the Manual Queue.
	await expect(
		list.getByRole('button', { name: 'Signal Part 2', exact: true }).first()
	).toBeFocused();
	await page.keyboard.press('Alt+ArrowUp');
	await page.keyboard.press('Alt+ArrowUp');
	await expect
		.poll(() => rows(list))
		.toEqual([
			'Queued by you 2',
			'Signal Remix',
			'Signal Part 2',
			'Up next 3',
			'Signal Remix',
			'Signal Part 1',
			'Signal Part 2'
		]);
});

test('dragging by the handle moves a song into the Manual Queue', async ({ page }) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	const all = list.locator('li.row[data-item]');
	await expect(all).toHaveCount(5);
	const [manual, ...context] = (await items(list)) as string[];

	// The context's last song to the top of the Manual Queue, just under its heading.
	await dragTo(page, all.nth(4), list, ROW + 4);
	await expect.poll(() => items(list)).toEqual([context[3], manual, ...context.slice(0, 3)]);
	await expect(list.getByRole('heading', { name: /Queued by you/ })).toContainText('2');
});

test('dragging one of several selected songs moves them all', async ({ page }) => {
	const panel = await openQueue(page, { queued: [P1, P2, R, P1, P2], start: 0 });
	const list = panel.getByRole('list', { name: 'Up Next' });
	const all = list.locator('li.row[data-item]');
	await expect(all).toHaveCount(4);
	const [a, b, c, d] = (await items(list)) as string[];

	// Space selects the song with focus; the first two are dragged to the end.
	await list.getByRole('button', { name: 'Signal Part 2', exact: true }).first().focus();
	await page.keyboard.press(' ');
	await page.keyboard.press('ArrowDown');
	await page.keyboard.press(' ');
	await expect(list.locator('li.selected')).toHaveCount(2);
	await dragTo(page, all.nth(0), list, ROW * 7);
	await expect.poll(() => items(list)).toEqual([c, d, a, b]);
});

test('a song is removed with its button or the Delete key', async ({ page }) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.locator('li.row[data-item]')).toHaveCount(5);

	await list.getByRole('button', { name: 'Remove Signal Part 1' }).click();
	await expect
		.poll(() => rows(list))
		.toEqual([
			'Queued by you 1',
			'Signal Remix',
			'Up next 3',
			'Signal Part 2',
			'Signal Remix',
			'Signal Part 2'
		]);
	await list.getByRole('button', { name: 'Signal Remix', exact: true }).nth(1).focus();
	await page.keyboard.press('Delete');
	await expect(list.locator('li.row[data-item]')).toHaveCount(3);
	// Focus moved on to the next song rather than being lost.
	await expect(
		list.getByRole('button', { name: 'Signal Part 2', exact: true }).nth(1)
	).toBeFocused();
});

test('clicking a song in Up Next jumps to it, and History can play it again or return it', async ({
	page
}) => {
	const panel = await openQueue(page, queued);
	const up = panel.getByRole('list', { name: 'Up Next' });
	await expect(up.locator('li.row[data-item]')).toHaveCount(5);

	// The third of Up Next, Part 1: everything ahead of it, the Manual Queue too, goes to History.
	await up.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	await expect
		.poll(() => rows(up))
		.toEqual([
			'Queued by you 0',
			'Nothing queued. Queue Last adds here.',
			'Up next 1',
			'Signal Part 2'
		]);
	await panel.getByRole('tab', { name: /History/ }).click();
	const history = panel.getByRole('list', { name: 'History' });
	// Most recent first: the playing Part 1 came first, then the manual Remix, Part 2, Remix.
	await expect
		.poll(() => rows(history))
		.toEqual(['Signal Remix', 'Signal Part 2', 'Signal Remix', 'Signal Part 1']);

	// Returned, a song goes to the front of Up Next without touching the rest of it: above
	// what was queued by hand since, which is added here, as another device would.
	const server = queueOf(page);
	server.edit({
		editId: crypto.randomUUID(),
		queueId: server.queueId,
		edit: {
			type: 'add',
			source: { key: crypto.randomUUID(), libraryId: '11', tracks: [P1] },
			before: { type: 'manualEnd' }
		}
	});
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	await expect(up.getByRole('heading', { name: /Queued by you/ })).toContainText('1');
	await panel.getByRole('tab', { name: /History/ }).click();
	await history.getByRole('button', { name: 'Return Signal Part 2 to Up Next' }).click();
	await expect(history.locator('li.row')).toHaveCount(3);
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	await expect
		.poll(() => rows(up))
		.toEqual(['Queued by you 2', 'Signal Part 2', 'Signal Part 1', 'Up next 1', 'Signal Part 2']);

	// Played again, it plays now: Up Next is as it was, and what played goes to History.
	await panel.getByRole('tab', { name: /History/ }).click();
	await history.getByRole('button', { name: 'Play Signal Remix again' }).first().click();
	await expect(history.locator('li.row')).toHaveCount(3);
	await expect(history.locator('li.row').first()).toContainText('Signal Part 1');
});

test('shuffle reorders the context and unshuffling puts it back', async ({ page }) => {
	const queued = Array.from({ length: 12 }, (_, i) => [P1, P2, R][i % 3]);
	const panel = await openQueue(page, { queued, start: 0 });
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.locator('li.row[data-item]')).toHaveCount(11);
	const before = await items(list);

	const shuffle = panel.getByRole('button', { name: 'Shuffle' });
	await shuffle.click();
	await expect(shuffle).toHaveAttribute('aria-pressed', 'true');
	await expect.poll(() => items(list)).not.toEqual(before);
	expect(new Set(await items(list))).toEqual(new Set(before));

	await shuffle.click();
	await expect(shuffle).toHaveAttribute('aria-pressed', 'false');
	await expect.poll(() => items(list)).toEqual(before);
});

test('repeat cycles Off, All, One and back', async ({ page }) => {
	const panel = await openQueue(page, undefined);
	await expect(panel.getByRole('button', { name: 'Repeat off' })).toBeVisible();
	await panel.getByRole('button', { name: 'Repeat off' }).click();
	await panel.getByRole('button', { name: 'Repeat all' }).click();
	await panel.getByRole('button', { name: 'Repeat one' }).click();
	await expect(panel.getByRole('button', { name: 'Repeat off' })).toBeVisible();
});

test('says so when a change did not apply because its place is gone', async ({ page }) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	const all = list.locator('li.row[data-item]');
	await expect(all).toHaveCount(5);
	const [, a, , , d] = (await items(list)) as string[];

	// Another device removed the song the drop would land before, and the page has not heard.
	queueOf(page).queue!.apply(JSON.stringify({ type: 'remove', items: [a] }));
	await dragTo(page, all.nth(4), list, ROW * 3 + 4);
	await expect(page.getByRole('status').filter({ hasText: 'did not apply' })).toContainText(
		'no longer in Up Next'
	);
	// The song is back where it was.
	await expect.poll(() => items(list)).toContain(d);
});

test('a queue of 50,000 songs draws only the rows in view', async ({ page }) => {
	const queued = Array.from({ length: 50_000 }, (_, i) => [P1, P2, R][i % 3]);
	const panel = await openQueue(page, { queued, start: 0 });
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.getByRole('heading', { name: /Up next/ })).toContainText('49,999');
	await expect(list.locator('li.row[data-item]').first()).toBeVisible();
	expect(await list.locator('li.row').count()).toBeLessThan(40);

	// Far down, other rows are drawn, and still only a screenful.
	const scroll = panel.locator('.scroll');
	await scroll.evaluate((el) => (el.scrollTop = el.scrollHeight));
	await expect(
		list.getByRole('button', { name: 'Signal Part 1', exact: true }).last()
	).toBeVisible();
	expect(await list.locator('li.row').count()).toBeLessThan(40);
	await expect(list.locator('li.row[data-item]').last()).toBeVisible();
});

test('the queue panel edits the queue of a player on this device too', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums/31');
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 1')).toBeVisible();

	await player.getByRole('button', { name: 'Queue' }).click();
	const list = page.getByRole('list', { name: 'Up Next' });
	await list.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();
});

test('a queue of 700,000 songs scrolls to its last row within a height every browser draws', async ({
	page
}) => {
	const queued = Array.from({ length: 700_000 }, (_, i) => [P1, P2, R][i % 3]);
	const panel = await openQueue(page, { queued, start: 0 });
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.locator('li.row[data-item]').first()).toBeVisible();

	const scroll = panel.locator('.scroll');
	const drawn = await list.evaluate((el) => el.getBoundingClientRect().height);
	// Past what Firefox draws a height of, so the list scrolls through a scaled range.
	expect(drawn).toBeLessThanOrEqual(10_000_000);
	await scroll.evaluate((el) => (el.scrollTop = el.scrollHeight));
	await expect(list.locator('li.row[data-item]').last()).toHaveAttribute('data-item', /\.699999$/);
	expect(await list.locator('li.row').count()).toBeLessThan(40);
	await expect(list.getByRole('listitem').last()).toHaveAttribute('aria-posinset', '699999');
});

test('a drop after scrolling lands at the line, though the rows below were still being read', async ({
	page
}) => {
	// Songs the library does not have, so every screenful is a lookup, which is slow here.
	const queued = Array.from({ length: 3000 }, (_, i) => String(1000 + i));
	const panel = await openQueue(page, { queued, start: 0 }, 150);
	const list = panel.getByRole('list', { name: 'Up Next' });
	const rowsDrawn = list.locator('li.row[data-item]');
	await expect(rowsDrawn.first()).toBeVisible();
	const moved = (await items(list))[0]!;

	const grip = (await rowsDrawn.first().locator('.grip').boundingBox())!;
	const box = (await panel.locator('.scroll').boundingBox())!;
	await page.mouse.move(grip.x + 10, grip.y + 10);
	await page.mouse.down();
	// Held at the bottom edge while it scrolls, then back to the middle and released.
	await page.mouse.move(grip.x + 10, box.y + box.height - 10, { steps: 6 });
	await page.waitForTimeout(700);
	await page.mouse.move(grip.x + 10, box.y + box.height / 2, { steps: 6 });
	await page.mouse.up();

	const where = () => queueOf(page).queue!.upcoming(0, 3000).indexOf(moved);
	await expect.poll(where).toBeGreaterThan(0);
	// Where the line was: a screenful or so in, not the end of the context.
	expect(await where()).toBeLessThan(150);
});

test('a selection does not outlive the songs it was made of reaching History', async ({ page }) => {
	const panel = await openQueue(page, { queued: [P1, P2, R, P1, P2], start: 0 });
	const list = panel.getByRole('list', { name: 'Up Next' });
	const all = list.locator('li.row[data-item]');
	await expect(all).toHaveCount(4);
	const history = panel.getByRole('tab', { name: /History/ });
	await expect(history).toContainText('0');

	// Select the first, then jump to the second: the first is in History now.
	await all.nth(0).getByRole('button').first().focus();
	await list.getByRole('button', { name: 'Signal Part 2', exact: true }).first().focus();
	await page.keyboard.press(' ');
	await list.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await expect(history).toContainText('2');

	// Dragging another song, selected the same way, takes nothing out of History.
	await expect(all).toHaveCount(2);
	await list.getByRole('button', { name: 'Signal Part 1', exact: true }).focus();
	await page.keyboard.press(' ');
	await dragTo(page, all.nth(0), list, ROW * 7);
	await expect.poll(async () => (await items(list)).length).toBe(2);
	await expect(history).toContainText('2');
});

test('pressing the handle without moving selects the row, and again lets it go', async ({
	page
}) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	const row = list.locator('li.row[data-item]').nth(2);
	const grip = (await row.locator('.grip').boundingBox())!;
	const press = async () => {
		await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
		await page.mouse.down();
		await page.mouse.up();
	};
	await press();
	await expect(row).toHaveClass(/selected/);
	await press();
	await expect(row).not.toHaveClass(/selected/);
});

test('Escape in the middle of a drag drops nothing and leaves the panel open', async ({ page }) => {
	const panel = await openQueue(page, queued);
	const list = panel.getByRole('list', { name: 'Up Next' });
	await expect(list.locator('li.row[data-item]')).toHaveCount(5);
	const before = await items(list);
	const row = list.locator('li.row[data-item]').nth(4);
	const grip = (await row.locator('.grip').boundingBox())!;
	await page.mouse.move(grip.x + 10, grip.y + 10);
	await page.mouse.down();
	await page.mouse.move(grip.x + 10, grip.y - 150, { steps: 5 });
	await page.keyboard.press('Escape');
	await page.mouse.up();
	await expect(panel).toBeVisible();
	expect(await items(list)).toEqual(before);
});
