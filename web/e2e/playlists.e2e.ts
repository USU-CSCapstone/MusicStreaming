import { expect, test } from '@playwright/test';
import { serveLibrary } from './fixtures';

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
