import { type Page, expect, test } from '@playwright/test';
import { serveLibrary } from './fixtures';

// Signal Part 1 is Ambient and Calm, Part 2 Ambient, and the Remix Electronic and Calm.

const titles = (page: Page) => page.locator('.track .title');

test('songs filter by tags of every type, held in the URL', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/songs');
	const songs = titles(page);
	await expect(songs).toHaveCount(3);

	await page.getByRole('button', { name: 'Genre' }).click();
	await page.getByRole('checkbox', { name: /Ambient/ }).check();
	await expect(page).toHaveURL(/tag=61/);
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Part 2', 'Signal Part 1']);

	await page.getByRole('button', { name: 'Mood' }).click();
	await page.getByRole('searchbox', { name: 'Find a mood' }).fill('cal');
	await page.getByRole('checkbox', { name: /Calm/ }).check();
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Part 1']);
	await expect(page.getByRole('heading', { name: 'Songs' })).toBeVisible();

	// A reload is the same filtered list, its tags named.
	await page.reload();
	await expect(songs).toHaveText(['Signal Part 1']);
	await expect(page.getByRole('button', { name: 'Remove genre Ambient' })).toBeVisible();

	await page.getByRole('button', { name: 'Clear filters' }).click();
	await expect(songs).toHaveCount(3);
	await expect(page).not.toHaveURL(/tag=/);
});

test("an album's tags lead to their pages, which play what carries them", async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();
	await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();

	await page.getByRole('link', { name: 'Calm' }).click();
	await expect(page).toHaveURL(/\/tags\/63$/);
	await expect(page.getByRole('heading', { level: 1, name: 'Calm' })).toBeVisible();
	await expect(page.getByText('2 songs · 1 album · 1 artist')).toBeVisible();
	const songs = titles(page);
	await expect(songs).toHaveText(['Signal Part 1', 'Signal Remix']);

	await page.getByRole('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(page.locator('.track.current .title')).toHaveText('Signal Part 1');
});

test('on a phone, a tag type opens as a sheet', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await serveLibrary(page);
	await page.goto('/songs');
	await page.getByRole('button', { name: 'Genre' }).click();
	const sheet = page.getByRole('dialog', { name: 'Genre filter' });
	await expect(sheet).toBeVisible();
	const box = (await sheet.boundingBox())!;
	expect(box.width).toBe(390);
	expect(box.y + box.height).toBeCloseTo(844, 0);

	await sheet.getByRole('checkbox', { name: /Electronic/ }).check();
	await sheet.getByRole('button', { name: 'Close genre filter' }).click();
	await expect(sheet).not.toBeVisible();
	await expect(titles(page)).toHaveText(['Signal Remix']);
});
