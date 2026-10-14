import { type Page, expect, test } from '@playwright/test';
import { longMix, serveLibrary, signalMix } from './fixtures';

test('creates a playlist and opens it', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/playlists');
	await expect(page.getByText('No playlists yet.')).toBeVisible();

	await page.getByRole('button', { name: 'New playlist' }).click();
	const dialog = page.getByRole('dialog', { name: 'New playlist' });
	const title = dialog.getByRole('textbox', { name: 'Title' });
	await expect(title).toBeFocused();
	// A title is required, and spaces are not one.
	await title.fill('   ');
	await expect(dialog.getByRole('button', { name: 'Create' })).toBeDisabled();

	await title.fill('Road Trip');
	await dialog.getByRole('textbox', { name: 'Description' }).fill('For the drive north.');
	await dialog.getByRole('button', { name: 'Create' }).click();

	await expect(page).toHaveURL(/\/playlists\/[^/]+$/);
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();
	await expect(page.getByText('For the drive north.')).toBeVisible();
	await expect(page.getByText('This playlist is empty.')).toBeVisible();

	await page.goBack();
	await expect(page.getByText('1 playlist', { exact: true })).toBeVisible();
	await expect(page.getByRole('link', { name: /Road Trip/ })).toBeVisible();
});

test('cancelling creates nothing', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/playlists');

	await page.getByRole('button', { name: 'New playlist' }).click();
	const dialog = page.getByRole('dialog', { name: 'New playlist' });
	await dialog.getByRole('textbox', { name: 'Title' }).fill('Never mind');
	await dialog.getByRole('button', { name: 'Cancel' }).click();
	await expect(dialog).toBeHidden();

	// Escape closes it too.
	await page.getByRole('button', { name: 'New playlist' }).click();
	await expect(dialog).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(dialog).toBeHidden();

	await expect(page.getByText('No playlists yet.')).toBeVisible();
	await expect(page).toHaveURL(/\/playlists$/);
});

test('edits a playlist’s details and pins it', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/playlists');
	await page.getByRole('button', { name: 'New playlist' }).click();
	const create = page.getByRole('dialog', { name: 'New playlist' });
	await create.getByRole('textbox', { name: 'Title' }).fill('Road Trip');
	await create.getByRole('textbox', { name: 'Description' }).fill('For the drive north.');
	await create.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();

	await page.getByRole('button', { name: 'Edit' }).click();
	const dialog = page.getByRole('dialog', { name: 'Edit details' });
	const title = dialog.getByRole('textbox', { name: 'Title' });
	await expect(title).toHaveValue('Road Trip');
	await title.fill('Road Trip 2026');
	// Clearing the description removes it.
	await dialog.getByRole('textbox', { name: 'Description' }).fill('');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toBeHidden();
	await expect(page.getByRole('heading', { name: 'Road Trip 2026' })).toBeVisible();
	await expect(page.getByText('For the drive north.')).toBeHidden();

	const pin = page.getByRole('button', { name: 'Pin' });
	await expect(pin).toHaveAttribute('aria-pressed', 'false');
	// Edits are sent one at a time, so the pin is the last to be saved.
	const saved = page.waitForResponse(
		(r) => r.request().method() === 'PATCH' && r.request().postDataJSON().pinned === true
	);
	await pin.click();
	await expect(pin).toHaveAttribute('aria-pressed', 'true');
	await saved;

	// Saved, not just shown.
	await page.reload();
	await expect(page.getByRole('heading', { name: 'Road Trip 2026' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Pin' })).toHaveAttribute('aria-pressed', 'true');
	await expect(page.getByText('For the drive north.')).toBeHidden();
});

test('cancelling an edit changes nothing', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/playlists');
	await page.getByRole('button', { name: 'New playlist' }).click();
	await page.getByRole('textbox', { name: 'Title' }).fill('Road Trip');
	await page.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();

	await page.getByRole('button', { name: 'Edit' }).click();
	const dialog = page.getByRole('dialog', { name: 'Edit details' });
	await dialog.getByRole('textbox', { name: 'Title' }).fill('Something else');
	await page.keyboard.press('Escape');
	await expect(dialog).toBeHidden();
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();
});

test('deletes a playlist, with a moment to undo it', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/playlists');
	await page.getByRole('button', { name: 'New playlist' }).click();
	await page.getByRole('textbox', { name: 'Title' }).fill('Road Trip');
	await page.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();

	// Undo brings it back, and nothing was sent.
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await expect(page).toHaveURL(/\/playlists$/);
	await expect(page.getByText('No playlists yet.')).toBeVisible();
	await expect(page.getByRole('status')).toHaveText(/Deleted “Road Trip”/);
	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.getByRole('link', { name: /Road Trip/ })).toBeVisible();
	await expect(page.getByText('1 playlist', { exact: true })).toBeVisible();

	// Dismissing the toast deletes it for good.
	await page.getByRole('link', { name: /Road Trip/ }).click();
	await page.getByRole('button', { name: 'Delete playlist' }).click();
	const sent = page.waitForResponse((r) => r.request().method() === 'DELETE');
	await page.getByRole('button', { name: 'Dismiss' }).click();
	expect((await sent).status()).toBe(204);
	await page.reload();
	await expect(page.getByText('No playlists yet.')).toBeVisible();

	// Back does not return to the deleted playlist.
	await page.goBack();
	await expect(page).toHaveURL(/\/playlists$/);
});

test('selects songs and removes them, with Undo', async ({ page }) => {
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	const row = (title: string) => page.getByRole('button', { name: title, exact: true });
	const bar = page.getByRole('toolbar', { name: 'Selected songs' });
	const titles = () => page.locator('.track .title');

	// Click, Shift-click a range, and Ctrl/Cmd-click one back out.
	await row('Signal Part 1').click();
	await row('Signal Remix').click({ modifiers: ['Shift'] });
	await expect(bar).toContainText('3 songs selected');
	await row('Signal Part 2').click({ modifiers: ['ControlOrMeta'] });
	await expect(bar).toContainText('2 songs selected');
	// Clicking a selected song lets go of it, and only it.
	await row('Signal Remix').click();
	await expect(bar).toContainText('1 song selected');
	await row('Signal Remix').click({ modifiers: ['ControlOrMeta'] });
	await expect(row('Signal Remix')).toHaveAccessibleDescription('Selected');

	await bar.getByRole('button', { name: 'Remove' }).click();
	await expect(titles()).toHaveText(['Signal Part 2']);
	await expect(page.getByText('1 song · ')).toBeVisible();
	await expect(bar).toBeHidden();
	await expect(page.getByRole('status')).toHaveText(/Removed 2 songs/);

	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(titles()).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);

	// Everything, and then nothing.
	await page.keyboard.press('ControlOrMeta+a');
	await expect(bar).toContainText('3 songs selected');
	await page.keyboard.press('Escape');
	await expect(bar).toBeHidden();

	// From the keyboard, Space on a title selects; the removal is saved.
	await row('Signal Remix').focus();
	await page.keyboard.press('Space');
	await expect(bar).toContainText('1 song selected');
	await expect(page.getByRole('region', { name: 'Player' }).getByText('Signal Remix')).toBeHidden();
	const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
	await bar.getByRole('button', { name: 'Remove' }).click();
	await saved;
	await page.reload();
	await expect(titles()).toHaveText(['Signal Part 1', 'Signal Part 2']);
});

test('a double click plays a song in a playlist, as a click there selects it', async ({ page }) => {
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).dblclick();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 2')).toBeVisible();
});

test('reorders with Alt+arrows and Move to…', async ({ page }) => {
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	const row = (title: string) => page.getByRole('button', { name: title, exact: true });
	const bar = page.getByRole('toolbar', { name: 'Selected songs' });
	const titles = () => page.locator('.track .title');

	// From the keyboard: the song stays selected and focused as it moves, and stops at the end.
	await row('Signal Part 1').focus();
	await page.keyboard.press('Space');
	await page.keyboard.press('Alt+ArrowDown');
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	await expect(row('Signal Part 1')).toBeFocused();
	await expect(row('Signal Part 1')).toHaveAccessibleDescription('Selected');
	await page.keyboard.press('Alt+ArrowDown');
	await page.keyboard.press('Alt+ArrowDown');
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Part 1']);
	await page.keyboard.press('Alt+ArrowUp');
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);

	// A scattered selection steps as it is; the top one cannot go higher, so it stays.
	await page.keyboard.press('Escape');
	await row('Signal Part 2').click();
	await row('Signal Remix').click({ modifiers: ['ControlOrMeta'] });
	await page.keyboard.press('Alt+ArrowUp');
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Part 1']);
	await expect(bar).toContainText('2 songs selected');

	// Move to… a position, and to the top.
	await page.keyboard.press('Escape');
	await row('Signal Part 1').click();
	await bar.getByRole('button', { name: 'Move to…' }).click();
	const dialog = page.getByRole('dialog', { name: 'Move 1 song' });
	const position = dialog.getByRole('spinbutton', { name: /Position/ });
	await expect(position).toHaveValue('3');
	await position.fill('2');
	await dialog.getByRole('button', { name: 'Move', exact: true }).click();
	await expect(dialog).toBeHidden();
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	await expect(bar).toContainText('1 song selected');

	await bar.getByRole('button', { name: 'Move to…' }).click();
	const saved = page.waitForResponse(
		(r) => r.request().method() === 'PATCH' && r.request().postDataJSON().ops?.[0].at === 0
	);
	await dialog.getByRole('button', { name: 'To the top' }).click();
	await expect(titles()).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);

	// Saved, in the order shown.
	await saved;
	await page.reload();
	await expect(titles()).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
});

test.describe('dragging', () => {
	const rows = (page: Page) => page.locator('.track');
	const titles = (page: Page) => page.locator('.track .title');

	/** Presses row `from`, by its duration unless `on` says where, ready to drag. */
	async function grab(page: Page, from: number, on = '.duration') {
		const box = (await rows(page).nth(from).locator(on).boundingBox())!;
		const at = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
		await page.mouse.move(at.x, at.y);
		await page.mouse.down();
		return at;
	}

	/** Just inside the top or bottom edge of row `n`. */
	async function edgeOf(page: Page, n: number, side: 'top' | 'bottom') {
		const box = (await rows(page).nth(n).boundingBox())!;
		return side === 'top' ? box.y + 2 : box.y + box.height - 2;
	}

	test('drags a song from anywhere on it, and saves the order', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		const at = await grab(page, 0, '.title');
		await page.mouse.move(at.x, await edgeOf(page, 2, 'bottom'), { steps: 8 });
		const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
		await page.mouse.up();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Part 1']);
		// The dragged song is the selection now.
		await expect(page.getByRole('toolbar', { name: 'Selected songs' })).toContainText('1 song');

		await saved;
		await page.reload();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Part 1']);
	});

	test('drags a scattered selection together, showing where it will land', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		await titles(page).nth(1).click();
		await titles(page)
			.nth(2)
			.click({ modifiers: ['ControlOrMeta'] });
		await titles(page)
			.nth(1)
			.click({ modifiers: ['ControlOrMeta'] });
		await titles(page)
			.nth(0)
			.click({ modifiers: ['ControlOrMeta'] });
		// Parts 1 and Remix selected; dragging Remix, by its album link, carries both.
		const at = await grab(page, 2, '.album-col a');
		await page.mouse.move(at.x, await edgeOf(page, 0, 'top'), { steps: 8 });
		await expect(page.locator('.drop')).toBeVisible();
		await expect(page.locator('.ghost')).toHaveText('2 songs');
		await expect(page.getByRole('toolbar', { name: 'Selected songs' })).toBeHidden();
		await page.mouse.up();
		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Remix', 'Signal Part 2']);
		await expect(page.locator('.drop')).toBeHidden();
		// A drag that starts on a link moves rows; it does not follow the link.
		await expect(page).toHaveURL(new RegExp(`/playlists/${signalMix.id}$`));
	});

	test('Escape, or a drop where it started, moves nothing', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		const edits: string[] = [];
		page.on('request', (r) => r.method() === 'PATCH' && edits.push(r.url()));

		let at = await grab(page, 0);
		await page.mouse.move(at.x, await edgeOf(page, 2, 'bottom'), { steps: 8 });
		await page.keyboard.press('Escape');
		await page.mouse.up();
		await expect(page.locator('.drop')).toBeHidden();

		at = await grab(page, 1);
		await page.mouse.move(at.x, at.y + 12, { steps: 4 });
		await page.mouse.up();

		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
		expect(edits).toEqual([]);
	});

	test('scrolls while held near the edge', async ({ page }) => {
		await page.setViewportSize({ width: 1280, height: 600 });
		await serveLibrary(page, { playlists: [longMix] });
		await page.goto(`/playlists/${longMix.id}`);
		const main = page.locator('main');
		const bottom = (await main.boundingBox())!;
		const at = await grab(page, 0);
		await page.mouse.move(at.x, bottom.y + bottom.height - 10, { steps: 8 });
		await expect.poll(() => main.evaluate((m) => m.scrollTop)).toBeGreaterThan(600);
		await page.mouse.up();
		// The first song left the top for well down the list: everything above it moved up one.
		const before = (await titles(page).allTextContents()).length;
		const rest = Array.from({ length: before - 1 }, (_, k) =>
			k % 3 === 2 ? 'Signal Part 1' : k % 3 === 0 ? 'Signal Part 2' : 'Signal Remix'
		);
		const order = await titles(page).allTextContents();
		const landed = order.findIndex((title, k) => title !== rest[k]);
		expect(landed).toBeGreaterThan(10);
		expect(order[landed]).toBe('Signal Part 1');
	});
});

test.describe('on a touch screen', () => {
	test.use({ hasTouch: true });

	test('drags a song after a long press', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		const rows = page.locator('.track');
		const start = (await rows.nth(0).locator('.duration').boundingBox())!;
		const target = (await rows.nth(2).boundingBox())!;
		const x = start.x + start.width / 2;
		const cdp = await page.context().newCDPSession(page);
		const touch = (type: string, y?: number) =>
			cdp.send('Input.dispatchTouchEvent', {
				type,
				touchPoints: y === undefined ? [] : [{ x, y }]
			});
		await touch('touchStart', start.y + start.height / 2);
		await page.waitForTimeout(700);
		for (let y = start.y + 20; y < target.y + target.height - 2; y += 12)
			await touch('touchMove', y);
		await touch('touchMove', target.y + target.height - 2);
		await touch('touchEnd');
		await expect(page.locator('.track .title')).toHaveText([
			'Signal Part 2',
			'Signal Remix',
			'Signal Part 1'
		]);
	});

	test('a tap plays, a long press selects, and then a tap selects', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		const row = (title: string) => page.getByRole('button', { name: title, exact: true });
		const bar = page.getByRole('toolbar', { name: 'Selected songs' });

		await row('Signal Part 2').tap();
		const player = page.getByRole('region', { name: 'Player' });
		await expect(player.getByText('Signal Part 2')).toBeVisible();
		await expect(bar).toBeHidden();

		// A finger held down, as Chrome reports it.
		const box = (await row('Signal Part 1').boundingBox())!;
		const touch = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
		const cdp = await page.context().newCDPSession(page);
		await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [touch] });
		await page.waitForTimeout(700);
		await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
		await expect(bar).toContainText('1 song selected');

		await row('Signal Remix').tap();
		await expect(bar).toContainText('2 songs selected');
		await row('Signal Part 1').tap();
		await expect(bar).toContainText('1 song selected');
		await expect(row('Signal Remix')).toHaveAccessibleDescription('Selected');
	});
});
