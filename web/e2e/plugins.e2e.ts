import { expect, test } from '@playwright/test';
import { pluginFile, serveLibrary } from './fixtures';

test('installs a plugin, approving some of its permissions', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Plugins' }).click();
	await expect(page).toHaveURL(/\/admin\/plugins$/);
	await expect(page.getByText('No plugins installed.')).toBeVisible();

	await page.locator('input[type=file]').setInputFiles(pluginFile());
	const dialog = page.getByRole('dialog', { name: 'Allow LRCLIB Lyrics' });
	await expect(dialog).toBeVisible();
	await expect(dialog.getByRole('checkbox', { name: 'Enable in Test Library' })).toBeDisabled();

	// Everything required, but not write access.
	await dialog.getByRole('checkbox', { name: /Network access/ }).check();
	await dialog.getByRole('checkbox', { name: /Read the library/ }).check();
	await dialog.getByRole('button', { name: 'Approve selected' }).click();
	await expect(dialog).toBeHidden();

	const card = page.getByRole('listitem').filter({ hasText: 'LRCLIB Lyrics' });
	await expect(card.getByRole('switch', { name: 'Enabled in Test Library' })).toBeChecked();
	await expect(card.getByText('Write to the library not granted')).toBeAttached();
	await expect(card.getByText('Network access granted')).toBeAttached();

	// Revoking a required permission disables it at once.
	await card.getByRole('button', { name: 'Permissions…' }).click();
	const edit = page.getByRole('dialog', { name: 'Permissions for LRCLIB Lyrics' });
	await edit.getByRole('checkbox', { name: /Network access/ }).uncheck();
	await edit.getByRole('button', { name: 'Approve selected' }).click();
	await expect(edit).toBeHidden();
	await expect(card.getByRole('switch', { name: 'Enabled in Test Library' })).not.toBeChecked();
	await expect(card.getByRole('switch', { name: 'Enabled in Test Library' })).toBeDisabled();
	await expect(card.getByText('Needs Network access.')).toBeVisible();

	await card.getByRole('button', { name: 'Uninstall' }).click();
	await card.getByRole('button', { name: 'Uninstall' }).last().click();
	await expect(page.getByText('No plugins installed.')).toBeVisible();
});

test('cancelling a fresh install removes the plugin', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/admin/plugins');
	await page.locator('input[type=file]').setInputFiles(pluginFile());
	await page.getByRole('button', { name: 'Cancel install' }).click();
	await expect(page.getByRole('dialog')).toBeHidden();
	await expect(page.getByText('No plugins installed.')).toBeVisible();
});

test('explains why a file will not install', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/admin/plugins');
	await page.locator('input[type=file]').setInputFiles({
		name: 'notes.wasm',
		mimeType: 'application/wasm',
		buffer: Buffer.from('not a plugin')
	});
	await expect(page.getByRole('alert')).toHaveText(/not a WebAssembly file/);
	await expect(page.getByRole('dialog')).toBeHidden();
});

test('runs an enabled plugin and shows what it did', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/admin/plugins');
	await page.locator('input[type=file]').setInputFiles(pluginFile());
	await page.getByRole('button', { name: 'Approve all' }).click();

	const card = page.getByRole('listitem').filter({ hasText: 'LRCLIB Lyrics' });
	await card.getByRole('button', { name: 'Run now' }).click();
	const result = card.getByRole('status');
	await expect(result).toContainText('Saved lyrics for 2 of 3 tracks');
	await expect(result).toContainText('scanner picks up the new files');
	await result.getByText('Log (2 lines)').click();
	await expect(result.getByText(/Signal Remix — Aurora Lane: not found/)).toBeVisible();
});

test('a disabled plugin offers nothing to run', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/admin/plugins');
	await page.locator('input[type=file]').setInputFiles(pluginFile());
	await page.getByRole('button', { name: 'Approve selected' }).click();
	const card = page.getByRole('listitem').filter({ hasText: 'LRCLIB Lyrics' });
	await expect(card.getByRole('switch')).not.toBeChecked();
	await expect(card.getByRole('button', { name: 'Run now' })).toHaveCount(0);
});
