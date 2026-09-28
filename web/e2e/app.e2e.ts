import { expect, test } from '@playwright/test';
import { serveLibrary } from './fixtures';

test('explains an empty server instead of erroring', async ({ page }) => {
	await serveLibrary(page, { empty: true });
	await page.goto('/');
	await expect(page.getByRole('heading', { name: 'No music yet' })).toBeVisible();
});

test('sidebar sections route to their views', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/');
	await expect(page).toHaveURL(/\/albums$/);
	const nav = page.getByRole('navigation', { name: 'Library' }).first();

	for (const [label, path] of [
		['Artists', '/artists'],
		['Playlists', '/playlists'],
		['Songs', '/songs'],
		['Albums', '/albums']
	]) {
		await nav.getByRole('link', { name: label }).click();
		await expect(page).toHaveURL(new RegExp(`${path}$`));
		await expect(page.getByRole('heading', { level: 1, name: label })).toBeVisible();
		await expect(nav.getByRole('link', { name: label })).toHaveAttribute('aria-current', 'page');
	}
});

test('grid and list view switch and are remembered', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await expect(page.locator('.grid')).toBeVisible();

	await page.getByRole('button', { name: 'List view' }).click();
	await expect(page.locator('.list')).toBeVisible();
	await expect(page.getByRole('button', { name: 'List view' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);

	await page.reload();
	await expect(page.locator('.list')).toBeVisible();
});

test('opening an album and clicking a song plays it', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();

	await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();
	await expect(page.getByText('Disc 1')).toBeVisible();
	await expect(page.getByText('Disc 2')).toBeVisible();

	await page.getByRole('button', { name: 'Signal Part 2' }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await expect(player.getByRole('button', { name: 'Pause' })).toBeVisible();

	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();
});

test('search updates as the user types', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('searchbox', { name: 'Search the library' })
		.first()
		.pressSequentially('remix');

	await expect(page).toHaveURL(/\/search\?q=remix$/);
	await expect(page.getByRole('button', { name: 'Signal Remix' })).toBeVisible();

	await page.getByRole('searchbox', { name: 'Search the library' }).first().fill('zzz');
	await expect(page.getByRole('heading', { name: /No music matches/ })).toBeVisible();
});
