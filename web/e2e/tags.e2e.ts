import type { Page } from '@playwright/test';
import { expect, test } from './test';

// Signal Part 1 is Ambient and Calm, Part 2 Ambient, and the Remix Electronic and Calm. Kindling,
// on another album, is Folk.

/** The song titles listed on the page, in order. */
const titles = (page: Page) => page.locator('.track .title');

test('songs filter by tags from their columns, held in the URL', async ({ page, server }) => {
	const { Ambient } = server.ids.tags;
	await page.goto('/songs');
	const songs = titles(page);
	await expect(songs).toHaveCount(4);
	// Each song's tags show in their columns, as links to their pages.
	const partTwo = page.locator('.track').filter({ hasText: 'Signal Part 2' });
	await expect(partTwo.getByRole('link', { name: 'Ambient' })).toHaveAttribute(
		'href',
		new RegExp(`/tags/${Ambient}$`)
	);

	await page.getByRole('button', { name: 'Filter by genre' }).click();
	await page.getByRole('checkbox', { name: /Ambient/ }).check();
	await expect(page).toHaveURL(new RegExp(`tagAny=${Ambient}`));
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
	await expect(songs).toHaveCount(4);
	await expect(page).not.toHaveURL(/tag(All|Any)=/);
});

test('the account chooses which tags are columns', async ({ page, server }) => {
	await page.goto('/account');
	await page.getByRole('checkbox', { name: 'Mood' }).uncheck();
	await page.goto('/songs');
	await expect(page.getByRole('button', { name: 'Filter by genre' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Filter by mood' })).toHaveCount(0);

	// A filter by a hidden type still shows its column, so it never narrows the list unseen.
	await page.goto(`/songs?tagAny=${server.ids.tags.Calm}`);
	await expect(page.getByRole('button', { name: 'Filter by mood, 1 chosen' })).toBeVisible();
	await expect(titles(page)).toHaveText(['Signal Remix', 'Signal Part 1']);
});

test("an album's tags lead to their pages, which play what carries them", async ({
	page,
	server
}) => {
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();
	await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();

	// The album's own tags, in its header; its songs' are in their columns, each under its
	// heading and clear of the next.
	const row = page.locator('.track').filter({ hasText: 'Signal Part 1' });
	const genre = (await row.getByRole('link', { name: 'Ambient' }).boundingBox())!;
	const mood = (await row.getByRole('link', { name: 'Calm' }).boundingBox())!;
	const moodHeading = (await page.getByRole('button', { name: 'Filter by mood' }).boundingBox())!;
	expect(genre.x + genre.width).toBeLessThanOrEqual(moodHeading.x);
	expect(mood.x).toBeGreaterThanOrEqual(moodHeading.x);

	await page.getByRole('group', { name: 'Mood' }).getByRole('link', { name: 'Calm' }).click();
	await expect(page).toHaveURL(new RegExp(`/tags/${server.ids.tags.Calm}$`));
	await expect(page.getByRole('heading', { level: 1, name: 'Calm' })).toBeVisible();
	await expect(page.getByText('2 songs · 1 album · 1 artist')).toBeVisible();
	const songs = titles(page);
	await expect(songs).toHaveText(['Signal Part 1', 'Signal Remix']);

	await page.getByRole('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(page.locator('.track.current .title')).toHaveText('Signal Part 1');
});

test('on a phone, a tag type opens as a sheet', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
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

test('a search filters from above its results, and keeps the filter when it leaves nothing', async ({
	page,
	server
}) => {
	const { Ambient, Electronic } = server.ids.tags;
	await page.goto('/search?q=signal');
	const songs = titles(page);
	await expect(songs).toHaveCount(3);
	const filters = page.getByRole('group', { name: 'Filter by tags' });

	await filters.getByRole('button', { name: 'Genre' }).click();
	await page.getByRole('checkbox', { name: /Electronic/ }).check();
	await page.getByRole('checkbox', { name: /Ambient/ }).check();
	await expect(songs).toHaveCount(3);
	// All of both genres is no song, though the album that holds a song of each still matches.
	await page.getByRole('radio', { name: 'All' }).click();
	await expect(page).toHaveURL(
		new RegExp(`tagAll=(${Electronic}%2C${Ambient}|${Ambient}%2C${Electronic})`)
	);
	await expect(songs).toHaveCount(0);
	await expect(page.getByRole('heading', { name: 'Albums' })).toBeVisible();
	// With a genre of the other album, nothing matches, and the filter stays to change.
	await page.getByRole('checkbox', { name: /Folk/ }).check();
	await expect(page.getByRole('heading', { name: /with these tags/ })).toBeVisible();
	await page.getByRole('checkbox', { name: /Folk/ }).uncheck();
	await page.getByRole('checkbox', { name: /Ambient/ }).uncheck();
	await page.keyboard.press('Escape');
	await expect(songs).toHaveText(['Signal Remix']);

	await filters.getByRole('button', { name: 'Clear filters' }).click();
	await expect(songs).toHaveCount(3);
});

test('on a phone, search leads to each tag type', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto('/search');
	await page
		.getByRole('navigation', { name: 'Browse tags' })
		.getByRole('link', { name: 'Moods' })
		.click();
	await expect(page.getByRole('heading', { level: 1, name: 'Moods' })).toBeVisible();
});

test('the sidebar has a section for each tag type, listing its tags', async ({ page, server }) => {
	await page.goto('/songs');
	const nav = page.getByRole('navigation', { name: 'Library' }).first();
	await nav.getByRole('link', { name: 'Genres' }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Genres' })).toBeVisible();
	await expect(page.getByText('3 genres')).toBeVisible();
	const main = page.getByRole('main');
	await expect(main.getByRole('link', { name: /Ambient/ })).toContainText('2 songs');

	await main.getByRole('link', { name: /Electronic/ }).click();
	await expect(page).toHaveURL(new RegExp(`/tags/${server.ids.tags.Electronic}$`));
	await expect(nav.getByRole('link', { name: 'Genres' })).toHaveAttribute('aria-current', 'page');

	await nav.getByRole('link', { name: 'Moods' }).click();
	await expect(main.getByRole('link', { name: /Calm/ })).toBeVisible();
});
