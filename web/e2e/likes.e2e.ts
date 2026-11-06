import { type Page, expect, test } from '@playwright/test';
import { album, serveLibrary } from './fixtures';

// Liking and hiding songs, against the mock's own store of them (`requirements/playlists.md`
// §11, `requirements/conventions.md` §8).

const heart = (page: Page, title: string) => page.getByRole('button', { name: `Like “${title}”` });
const row = (page: Page, title: string) =>
	page.locator('main').getByRole('button', { name: title, exact: true });

/** Opens Liked Songs from the playlists page. */
async function openLikedSongs(page: Page) {
	await page.goto('/playlists');
	await page.getByRole('link', { name: /^Liked Songs/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Liked Songs' })).toBeVisible();
}

test('a liked song tops Liked Songs, and unliked it leaves', async ({ page }) => {
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await heart(page, 'Signal Part 2').click();
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	await heart(page, 'Signal Remix').click();

	await openLikedSongs(page);
	await expect(page.getByText('2 songs · ')).toBeVisible();
	await expect(page.locator('.track .title')).toHaveText(['Signal Remix', 'Signal Part 2']);
	await heart(page, 'Signal Remix').click();
	await expect(row(page, 'Signal Remix')).toBeHidden();

	// It stays liked as the album shows it again.
	await page.goto(`/albums/${album.id}`);
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	await expect(heart(page, 'Signal Remix')).toHaveAttribute('aria-pressed', 'false');
});

test('a hidden song leaves every list until it is unhidden from Hidden Songs', async ({ page }) => {
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
	await page.getByRole('menuitem', { name: 'Hide song' }).click();
	await expect(row(page, 'Signal Part 2')).toBeHidden();
	await expect(page.getByRole('status')).toHaveText(/Hid “Signal Part 2”/);

	await page.goto('/songs');
	await expect(row(page, 'Signal Part 1')).toBeVisible();
	await expect(row(page, 'Signal Part 2')).toBeHidden();

	await page.goto('/account');
	await page.getByRole('link', { name: /Hidden Songs · 1/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Hidden Songs' })).toBeVisible();
	// A list to unhide from, not to play.
	await expect(page.locator('main').getByRole('button', { name: 'Play' })).toBeHidden();
	await page.getByRole('button', { name: 'Unhide “Signal Part 2”' }).click();
	await expect(page.getByText(/^You haven't hidden any songs/)).toBeVisible();

	await page.goto('/songs');
	await expect(row(page, 'Signal Part 2')).toBeVisible();
});

test('liking a hidden song unhides it', async ({ page }) => {
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
	await page.getByRole('menuitem', { name: 'Hide song' }).click();
	await expect(row(page, 'Signal Part 2')).toBeHidden();

	// The whole album into Liked Songs, the hidden song with it.
	await page.getByRole('button', { name: 'Actions for “Signal”' }).click();
	const added = page.waitForResponse((r) => r.request().method() === 'PATCH');
	await page.getByRole('menuitem', { name: /^Liked Songs/ }).click();
	await expect(page.getByRole('status')).toHaveText(/Added “Signal” to “Liked Songs”/);
	expect((await added).status()).toBe(200);

	await page.goto('/account/hidden');
	await expect(page.getByText(/^You haven't hidden any songs/)).toBeVisible();
	await openLikedSongs(page);
	await expect(row(page, 'Signal Part 2')).toBeVisible();
});

test('deleting Liked Songs with songs in it asks first, and unlikes them', async ({ page }) => {
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await heart(page, 'Signal Part 1').click();
	await openLikedSongs(page);

	await page.getByRole('button', { name: 'Delete playlist' }).click();
	const dialog = page.getByRole('dialog', { name: 'Delete Liked Songs?' });
	await expect(dialog).toContainText('Its song will be unliked.');
	await dialog.getByRole('button', { name: 'Cancel' }).click();
	await expect(dialog).toBeHidden();
	await expect(page).toHaveURL(/\/playlists\/[^/]+$/);

	await page.getByRole('button', { name: 'Delete playlist' }).click();
	await dialog.getByRole('button', { name: 'Delete' }).click();
	await expect(page).toHaveURL(/\/playlists$/);
	const sent = page.waitForResponse((r) => r.request().method() === 'DELETE');
	await page.getByRole('button', { name: 'Dismiss' }).click();
	expect((await sent).status()).toBe(204);

	await page.goto(`/albums/${album.id}`);
	await expect(heart(page, 'Signal Part 1')).toHaveAttribute('aria-pressed', 'false');
	// The next like brings it back.
	await heart(page, 'Signal Part 1').click();
	await page.goto('/playlists');
	await expect(page.getByRole('link', { name: /^Liked Songs/ })).toBeVisible();
});
