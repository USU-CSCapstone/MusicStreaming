import { type Page, expect, test } from '@playwright/test';
import { album, artist, longMix, roadTrip, serveLibrary, signalMix } from './fixtures';

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

test('reorders with Alt+arrows', async ({ page }) => {
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	const row = (title: string) => page.getByRole('button', { name: title, exact: true });
	const bar = page.getByRole('toolbar', { name: 'Selected songs' });
	const titles = () => page.locator('.track .title');
	let saves = 0;
	page.on('response', (r) => r.request().method() === 'PATCH' && saves++);

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

	// Saved, in the order shown: four moves, as the press at the bottom moved nothing.
	await expect.poll(() => saves).toBe(4);
	await page.reload();
	await expect(titles()).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Part 1']);
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

test.describe('adding to a playlist', () => {
	const titles = (page: Page) => page.locator('.track .title');
	const more = (page: Page, title: string) =>
		page.getByRole('button', { name: `Add ${title} to a playlist` });

	test('adds a song from its ⋯, and opens the playlist', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix, roadTrip] });
		await page.goto(`/albums/${album.id}`);
		await more(page, 'Signal Part 2').click();
		const menu = page.getByRole('menu', { name: 'Add “Signal Part 2” to a playlist' });
		await menu.getByRole('menuitem', { name: /Road Trip/ }).click();
		await expect(menu).toBeHidden();
		await expect(page.getByRole('status')).toHaveText(/Added “Signal Part 2” to “Road Trip”/);

		await page.getByRole('button', { name: 'Open' }).click();
		await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();
		await expect(titles(page)).toHaveText(['Signal Part 2']);
	});

	test('warns about a song already there, and adds it only if asked', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/albums/${album.id}`);
		const dialog = page.getByRole('dialog', { name: 'Already in this playlist' });

		await more(page, 'Signal Part 1').click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		await expect(dialog).toContainText(
			'“Signal Part 1” is already in “Signal Mix”. Add it anyway?'
		);
		await dialog.getByRole('button', { name: 'Cancel' }).click();
		await expect(dialog).toBeHidden();

		await more(page, 'Signal Part 1').click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
		await dialog.getByRole('button', { name: 'Add anyway' }).click();
		await saved;
		await page.goto(`/playlists/${signalMix.id}`);
		await expect(titles(page)).toHaveText([
			'Signal Part 1',
			'Signal Part 2',
			'Signal Remix',
			'Signal Part 1'
		]);
	});

	test('makes a new playlist with the song in it', async ({ page }) => {
		await serveLibrary(page);
		await page.goto(`/albums/${album.id}`);
		await more(page, 'Signal Remix').click();
		await page.getByRole('menuitem', { name: 'New playlist…' }).click();
		const dialog = page.getByRole('dialog', { name: 'New playlist' });
		await dialog.getByRole('textbox', { name: 'Title' }).fill('Favorites');
		await dialog.getByRole('button', { name: 'Create' }).click();
		await expect(dialog).toBeHidden();
		await expect(page.getByRole('status')).toHaveText(/Added “Signal Remix” to “Favorites”/);
		// Still on the album.
		await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();

		await page.goto('/playlists');
		await expect(page.getByRole('link', { name: /Favorites/ })).toContainText('1 song');
	});

	test('adds the selection, skipping songs already there if asked', async ({ page }) => {
		await serveLibrary(page, {
			playlists: [signalMix, { ...roadTrip, trackIds: [BigInt(signalMix.trackIds[1])] }]
		});
		await page.goto(`/playlists/${signalMix.id}`);
		await expect(titles(page)).toHaveCount(3);
		await page.keyboard.press('ControlOrMeta+a');
		const bar = page.getByRole('toolbar', { name: 'Selected songs' });
		await bar.getByRole('button', { name: 'Add to playlist' }).click();
		await page.getByRole('menuitem', { name: /Road Trip/ }).click();

		const dialog = page.getByRole('dialog', { name: 'Already in this playlist' });
		await expect(dialog).toContainText('1 of these 3 songs is already in “Road Trip”.');
		await dialog.getByRole('button', { name: 'Skip duplicates' }).click();
		await expect(page.getByRole('status')).toHaveText(/Added 2 songs to “Road Trip”/);
		// The selection is still there, for whatever comes next.
		await expect(bar).toContainText('3 songs selected');

		await page.getByRole('button', { name: 'Open' }).click();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	});

	test('adds to the playlist open on screen at once', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		await more(page, 'Signal Remix').click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		await page.getByRole('button', { name: 'Add anyway' }).click();
		await expect(titles(page)).toHaveText([
			'Signal Part 1',
			'Signal Part 2',
			'Signal Remix',
			'Signal Remix'
		]);
		await expect(page.getByText('4 songs · ')).toBeVisible();

		// Its next edit is not taken for a change made elsewhere.
		await titles(page).first().click();
		const saved = page.waitForResponse(
			(r) => r.request().method() === 'PATCH' && !!r.request().postDataJSON().ops?.[0].positions
		);
		await page.getByRole('toolbar').getByRole('button', { name: 'Remove' }).click();
		expect((await saved).status()).toBe(200);
		await expect(page.getByRole('status')).not.toHaveText(/another device/);
		await page.reload();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Remix', 'Signal Remix']);
	});

	test('works from the keyboard', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix, roadTrip] });
		await page.goto(`/albums/${album.id}`);
		await more(page, 'Signal Part 1').focus();
		await page.keyboard.press('Enter');
		await expect(page.getByRole('menuitem', { name: 'New playlist…' })).toBeFocused();
		await expect(page.getByRole('menuitem')).toHaveCount(3);
		await page.keyboard.press('ArrowDown');
		await expect(page.getByRole('menuitem', { name: /Signal Mix/ })).toBeFocused();
		await page.keyboard.press('ArrowUp');
		await page.keyboard.press('ArrowUp');
		await expect(page.getByRole('menuitem', { name: /Road Trip/ })).toBeFocused();
		await page.keyboard.press('Escape');
		await expect(page.getByRole('menu')).toBeHidden();
		await expect(more(page, 'Signal Part 1')).toBeFocused();
	});
});

test.describe('adding a whole album, artist, or playlist', () => {
	const titles = (page: Page) => page.locator('.track .title');
	/** The ops the next edit sends. */
	const nextEdit = (page: Page) =>
		page
			.waitForRequest((r) => r.method() === 'PATCH')
			.then((r) => r.postDataJSON().ops as { items: unknown[] }[]);

	test('adds an album, which the server expands', async ({ page }) => {
		await serveLibrary(page, { playlists: [roadTrip] });
		await page.goto(`/albums/${album.id}`);
		await page.getByRole('button', { name: 'Add “Signal” to a playlist' }).click();
		const sent = nextEdit(page);
		await page.getByRole('menuitem', { name: /Road Trip/ }).click();
		expect((await sent)[0].items).toEqual([{ type: 'album', id: album.id }]);
		await expect(page.getByRole('status')).toHaveText(/Added “Signal” to “Road Trip”/);

		await page.getByRole('button', { name: 'Open' }).click();
		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
	});

	test('skips the songs of an album already there, if asked', async ({ page }) => {
		await serveLibrary(page, {
			playlists: [{ ...roadTrip, trackIds: [BigInt(signalMix.trackIds[1])] }]
		});
		await page.goto(`/albums/${album.id}`);
		await page.getByRole('button', { name: 'Add “Signal” to a playlist' }).click();
		await page.getByRole('menuitem', { name: /Road Trip/ }).click();
		const dialog = page.getByRole('dialog', { name: 'Already in this playlist' });
		await expect(dialog).toContainText(
			'1 of the 3 songs in “Signal” is already in “Road Trip”. Add them anyway?'
		);
		const sent = nextEdit(page);
		await dialog.getByRole('button', { name: 'Skip duplicates' }).click();
		// Only the others, by name, as the album would bring the repeat with it.
		expect((await sent)[0].items).toEqual([
			{ type: 'track', id: '41' },
			{ type: 'track', id: '43' }
		]);
		await expect(page.getByRole('status')).toHaveText(/Added 2 songs to “Road Trip”/);
		await page.getByRole('button', { name: 'Open' }).click();
		await expect(titles(page)).toHaveText(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	});

	test('makes a new playlist from an artist', async ({ page }) => {
		await serveLibrary(page);
		await page.goto(`/artists/${artist.id}`);
		await page.getByRole('button', { name: 'Add “Aurora Lane” to a playlist' }).click();
		await page.getByRole('menuitem', { name: 'New playlist…' }).click();
		const dialog = page.getByRole('dialog', { name: 'New playlist' });
		await dialog.getByRole('textbox', { name: 'Title' }).fill('All of Aurora');
		const created = page.waitForRequest(
			(r) => r.method() === 'POST' && r.url().endsWith('/playlists')
		);
		await dialog.getByRole('button', { name: 'Create' }).click();
		expect((await created).postDataJSON().items).toEqual([{ type: 'artist', id: artist.id }]);
		await expect(page.getByRole('status')).toHaveText(/Added “Aurora Lane” to “All of Aurora”/);

		await page.goto('/playlists');
		await expect(page.getByRole('link', { name: /All of Aurora/ })).toContainText('3 songs');
	});

	test('adds a playlist to another, or to itself at once', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix, roadTrip] });
		await page.goto(`/playlists/${signalMix.id}`);
		const more = page.getByRole('button', { name: 'Add “Signal Mix” to a playlist' });

		await more.click();
		const sent = nextEdit(page);
		await page.getByRole('menuitem', { name: /Road Trip/ }).click();
		expect((await sent)[0].items).toEqual([{ type: 'playlist', id: signalMix.id }]);

		await more.click();
		await page.getByRole('menuitem', { name: /Signal Mix/ }).click();
		const dialog = page.getByRole('dialog', { name: 'Already in this playlist' });
		await expect(dialog).toContainText('All of “Signal Mix” is already in “Signal Mix”.');
		await expect(dialog.getByRole('button', { name: 'Skip duplicates' })).toBeHidden();
		await dialog.getByRole('button', { name: 'Add anyway' }).click();
		await expect(titles(page)).toHaveCount(6);
		await expect(page.getByText('6 songs · ')).toBeVisible();

		await page.goto(`/playlists/${roadTrip.id}`);
		await expect(titles(page)).toHaveText(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);
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
