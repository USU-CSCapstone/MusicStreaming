import { type Page, expect, test } from '@playwright/test';
import { album, serveLibrary, signalMix } from './fixtures';

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

test('pressing a heart keeps the songs selected', async ({ page }) => {
	await serveLibrary(page, { playlists: [signalMix] });
	await page.goto(`/playlists/${signalMix.id}`);
	const bar = page.getByRole('toolbar', { name: 'Selected songs' });
	await row(page, 'Signal Part 1').click();
	await row(page, 'Signal Part 2').click({ modifiers: ['Shift'] });
	await expect(bar).toContainText('2 songs selected');

	await heart(page, 'Signal Part 2').click();
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
	await heart(page, 'Signal Remix').click();
	await expect(heart(page, 'Signal Remix')).toHaveAttribute('aria-pressed', 'true');
	await expect(bar).toContainText('2 songs selected');
	await expect(row(page, 'Signal Part 1')).toHaveAccessibleDescription('Selected');
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

test('an album added to Liked Songs leaves its hidden song hidden', async ({ page }) => {
	await serveLibrary(page);
	await page.goto(`/albums/${album.id}`);
	await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
	await page.getByRole('menuitem', { name: 'Hide song' }).click();
	await expect(row(page, 'Signal Part 2')).toBeHidden();

	// The album as its page shows it, without the hidden song, which no like unhides.
	await page.getByRole('button', { name: 'Actions for “Signal”' }).click();
	const added = page.waitForResponse((r) => r.request().method() === 'PATCH');
	await page.getByRole('menuitem', { name: /^Liked Songs/ }).click();
	await expect(page.getByRole('status')).toHaveText(/Added “Signal” to “Liked Songs”/);
	expect((await added).status()).toBe(200);

	await openLikedSongs(page);
	await expect(row(page, 'Signal Part 1')).toBeVisible();
	await expect(row(page, 'Signal Part 2')).toBeHidden();
	await page.goto('/account/hidden');
	await expect(page.getByRole('button', { name: 'Unhide “Signal Part 2”' })).toBeVisible();
});
