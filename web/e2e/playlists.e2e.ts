import { expect, test } from '@playwright/test';
import { serveLibrary, signalMix } from './fixtures';

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

	// The boxes select too, as on a touch screen; the removal is saved.
	await page.getByRole('checkbox', { name: 'Select Signal Remix' }).check();
	await expect(bar).toContainText('1 song selected');
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

test.describe('on a touch screen', () => {
	test.use({ hasTouch: true });

	test('a tap plays until a box is checked, and then selects', async ({ page }) => {
		await serveLibrary(page, { playlists: [signalMix] });
		await page.goto(`/playlists/${signalMix.id}`);
		const bar = page.getByRole('toolbar', { name: 'Selected songs' });

		await page.getByRole('button', { name: 'Signal Part 2', exact: true }).tap();
		const player = page.getByRole('region', { name: 'Player' });
		await expect(player.getByText('Signal Part 2')).toBeVisible();
		await expect(bar).toBeHidden();

		await page.getByRole('checkbox', { name: 'Select Signal Part 1' }).tap();
		await page.getByRole('button', { name: 'Signal Remix', exact: true }).tap();
		await expect(bar).toContainText('2 songs selected');
		await expect(page.getByRole('checkbox', { name: 'Select Signal Remix' })).toBeChecked();
	});
});
