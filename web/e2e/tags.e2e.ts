import { type Page, expect, test } from '@playwright/test';
import { serveLibrary } from './fixtures';

// Signal Part 1 is Ambient and Calm, Part 2 Ambient, and the Remix Electronic and Calm.

const titles = (page: Page) => page.locator('.track .title');

test('songs filter by tags from their columns, held in the URL', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/songs');
	const songs = titles(page);
	await expect(songs).toHaveCount(3);
	// Each song's tags show in their columns, as links to their pages.
	const partTwo = page.locator('.track').filter({ hasText: 'Signal Part 2' });
	await expect(partTwo.getByRole('link', { name: 'Ambient' })).toHaveAttribute(
		'href',
		/\/tags\/61$/
	);

	await page.getByRole('button', { name: 'Filter by genre' }).click();
	await page.getByRole('checkbox', { name: /Ambient/ }).check();
	await expect(page).toHaveURL(/tag=61/);
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Part 2', 'Signal Part 1']);

	await page.getByRole('button', { name: 'Filter by mood' }).click();
	await page.getByRole('searchbox', { name: 'Find a mood' }).fill('cal');
	await page.getByRole('checkbox', { name: /Calm/ }).check();
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Part 1']);

	// A reload is the same filtered list, each heading saying what it holds.
	await page.reload();
	await expect(songs).toHaveText(['Signal Part 1']);
	await expect(page.getByRole('button', { name: 'Filter by genre, 1 chosen' })).toBeVisible();

	for (const type of ['genre', 'mood']) {
		await page.getByRole('button', { name: `Filter by ${type}, 1 chosen` }).click();
		await page.getByRole('button', { name: 'Clear', exact: true }).click();
		await page.keyboard.press('Escape');
	}
	await expect(songs).toHaveCount(3);
	await expect(page).not.toHaveURL(/tag=/);
});

test('the account chooses which tags are columns', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/account');
	await page.getByRole('checkbox', { name: 'Mood' }).uncheck();
	await page.goto('/songs');
	await expect(page.getByRole('button', { name: 'Filter by genre' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Filter by mood' })).toHaveCount(0);

	// A filter by a hidden type still shows its column, so it never narrows the list unseen.
	await page.goto('/songs?tag=63');
	await expect(page.getByRole('button', { name: 'Filter by mood, 1 chosen' })).toBeVisible();
	await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 1']);
});

test("an album's tags lead to their pages, which play what carries them", async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();
	await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();

	// The album's own tags, in its header; its songs' are in their columns.
	await page.getByRole('group', { name: 'Mood' }).getByRole('link', { name: 'Calm' }).click();
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

test('the sidebar has a section for each tag type, listing its tags', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/songs');
	const nav = page.getByRole('navigation', { name: 'Library' }).first();
	await nav.getByRole('link', { name: 'Genres' }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Genres' })).toBeVisible();
	await expect(page.getByText('2 genres')).toBeVisible();
	const main = page.getByRole('main');
	await expect(main.getByRole('link', { name: /Ambient/ })).toContainText('2 songs');

	await main.getByRole('link', { name: /Electronic/ }).click();
	await expect(page).toHaveURL(/\/tags\/62$/);
	await expect(nav.getByRole('link', { name: 'Genres' })).toHaveAttribute('aria-current', 'page');

	await nav.getByRole('link', { name: 'Moods' }).click();
	await expect(main.getByRole('link', { name: /Calm/ })).toBeVisible();
});
