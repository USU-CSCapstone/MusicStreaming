import type { Page } from '@playwright/test';
import { expect, test } from './test';
import { TEST_PLUGIN } from './server';

/** Opens the Plugins page and gives the plugin file to the install box. */
async function startInstall(page: Page) {
	await page.goto('/admin/plugins');
	await page.locator('input[type=file]').setInputFiles(TEST_PLUGIN);
}

test('installs a plugin, approving some of its permissions', async ({ page, server }) => {
	const libraryName = (await server.api('/libraries')).items[0].name;
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Plugins' }).click();
	await expect(page).toHaveURL(/\/admin\/plugins$/);
	await expect(page.getByText('No plugins installed.')).toBeVisible();

	await page.locator('input[type=file]').setInputFiles(TEST_PLUGIN);
	const dialog = page.getByRole('dialog', { name: 'Allow Test Plugin' });
	await expect(dialog).toBeVisible();
	await expect(dialog.getByRole('checkbox', { name: `Enable in ${libraryName}` })).toBeDisabled();

	// Everything required, but not write access.
	await dialog.getByRole('checkbox', { name: /Network access/ }).check();
	await dialog.getByRole('checkbox', { name: /Read the library/ }).check();
	await dialog.getByRole('button', { name: 'Approve selected' }).click();
	await expect(dialog).toBeHidden();

	const card = page.getByRole('listitem').filter({ hasText: 'Test Plugin' });
	const enabled = card.getByRole('switch', { name: `Enabled in ${libraryName}` });
	await expect(enabled).toBeChecked();
	await expect(card.getByText('Add files to the library not granted')).toBeAttached();
	await expect(card.getByText('Network access granted')).toBeAttached();

	// Revoking a required permission disables it at once.
	await card.getByRole('button', { name: 'Permissions…' }).click();
	const edit = page.getByRole('dialog', { name: 'Permissions for Test Plugin' });
	await edit.getByRole('checkbox', { name: /Network access/ }).uncheck();
	await edit.getByRole('button', { name: 'Approve selected' }).click();
	await expect(edit).toBeHidden();
	await expect(enabled).not.toBeChecked();
	await expect(enabled).toBeDisabled();
	await expect(card.getByText('Needs Network access.')).toBeVisible();

	await card.getByRole('button', { name: 'Uninstall' }).click();
	await card.getByRole('button', { name: 'Uninstall' }).last().click();
	await expect(page.getByText('No plugins installed.')).toBeVisible();
});

test('cancelling a fresh install removes the plugin', async ({ page }) => {
	await startInstall(page);
	await page.getByRole('button', { name: 'Cancel install' }).click();
	await expect(page.getByRole('dialog')).toBeHidden();
	await expect(page.getByText('No plugins installed.')).toBeVisible();
});

test('explains why a file will not install', async ({ page }) => {
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
	await startInstall(page);
	await page.getByRole('button', { name: 'Approve all' }).click();

	const card = page.getByRole('listitem').filter({ hasText: 'Test Plugin' });
	await card.getByRole('button', { name: 'Run now' }).click();
	const result = card.getByRole('status');
	await expect(result).toContainText('Saved a note beside 4 tracks.');
	await expect(result).toContainText('scanner picks up the new files');
	await result.getByText('Log (2 lines)').click();
	await expect(result.getByText('Counted 4 tracks')).toBeVisible();
	await expect(result.getByText(/Saved e2e-test-plugin-note-\d+\.txt/)).toBeVisible();
});

test('a disabled plugin offers nothing to run', async ({ page }) => {
	await startInstall(page);
	await page.getByRole('button', { name: 'Approve selected' }).click();
	const card = page.getByRole('listitem').filter({ hasText: 'Test Plugin' });
	await expect(card.getByRole('switch')).not.toBeChecked();
	await expect(card.getByRole('button', { name: 'Run now' })).toHaveCount(0);
});

test('settings are saved, and a secret is never shown again', async ({ page }) => {
	await startInstall(page);
	const allow = page.getByRole('dialog', { name: 'Allow Test Plugin' });
	await allow.getByRole('button', { name: 'Approve all' }).click();
	await expect(allow).toBeHidden();

	const card = page.getByRole('listitem').filter({ hasText: 'Test Plugin' });
	await card.getByRole('button', { name: 'Settings…' }).click();
	const dialog = page.getByRole('dialog', { name: 'Settings for Test Plugin' });
	await dialog.getByLabel('Verbose log').check();
	await dialog.getByLabel('API key').fill('s3cret');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toBeHidden();

	await card.getByRole('button', { name: 'Settings…' }).click();
	await expect(dialog.getByLabel('Verbose log')).toBeChecked();
	await expect(dialog.getByLabel('API key')).toHaveValue('');
	await expect(dialog.getByLabel('API key')).toHaveAttribute(
		'placeholder',
		'Saved; type to replace it'
	);
});

test('connects a plugin with its own token on the account page, and disconnects it', async ({
	page,
	server
}) => {
	await startInstall(page);
	await page.getByRole('button', { name: 'Approve all' }).click();
	await expect(page.getByRole('dialog')).toBeHidden();

	await page.goto('/account');
	const connections = page.getByRole('region', { name: 'Connections' });
	await connections.getByRole('button', { name: 'Connect…' }).click();

	const dialog = page.getByRole('dialog', { name: 'Connect Test Plugin' });
	await dialog.getByLabel('User token').fill('wrong-token');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog.getByRole('alert')).toHaveText(
		'the plugin reported an error: The test plugin refused this token.'
	);
	await dialog.getByLabel('User token').fill('good-token');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toBeHidden();
	await expect(connections.getByText('Connected')).toBeVisible();

	// Searches are shared only once the user turns it on, and stay so after a reload.
	const share = connections.getByRole('checkbox', { name: /Share my searches with Test Plugin/ });
	await expect(share).not.toBeChecked();
	await share.check();
	// It shows at once and is sent after, so the reload waits for the server to hold it.
	await expect
		.poll(async () => (await server.api('/me/plugins')).items[0]?.sharesSearches)
		.toBe(true);
	await page.reload();
	await expect(share).toBeChecked();

	await connections.getByRole('button', { name: 'Disconnect' }).click();
	await expect(connections.getByRole('button', { name: 'Connect…' })).toBeVisible();
	await expect(share).toHaveCount(0);
});
