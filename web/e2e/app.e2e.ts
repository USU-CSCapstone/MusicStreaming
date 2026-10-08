import { expect, test } from '@playwright/test';
import { PASSWORD, serveLibrary } from './fixtures';

test('explains an empty server instead of erroring', async ({ page }) => {
	await serveLibrary(page, { empty: true });
	await page.goto('/');
	await expect(page.getByRole('heading', { name: 'No music yet' })).toBeVisible();
});

test('explains to a user with no library that they need access', async ({ page }) => {
	await serveLibrary(page, { empty: true, role: 'user' });
	await page.goto('/');
	await expect(
		page.getByText("You don't have access to a library on this server yet.")
	).toBeVisible();
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
	const { plays } = await serveLibrary(page);
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

	// A play is reported as it starts and again as it ends, with the album it was played from.
	// Its two seconds may have ended before Next was pressed.
	const ends = () => plays.filter((p) => p.playId === plays[0]?.playId).map((p) => p.end);
	await expect.poll(ends).toEqual([null, expect.stringMatching(/^(skipped|finished)$/)]);
	expect(plays[0]).toMatchObject({ context: { type: 'album' }, origin: 'context' });
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

test('a search played from is offered again when the box is empty, and can be removed', async ({
	page
}) => {
	const { searches } = await serveLibrary(page);
	await page.goto('/albums');
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	await box.pressSequentially('remix');
	await page.getByRole('button', { name: 'Signal Remix' }).click();
	await expect
		.poll(() => searches.map((s) => [s.query, s.selected]))
		.toEqual([['remix', { type: 'track', id: '43' }]]);

	await box.fill('');
	const recent = page.getByRole('region', { name: 'Recent searches' });
	await recent.getByRole('button', { name: 'remix', exact: true }).click();
	await expect(page).toHaveURL(/\/search\?q=remix$/);

	await box.fill('');
	await recent.getByRole('button', { name: 'Remove “remix” from recent searches' }).click();
	await expect(recent).toHaveCount(0);
	await expect.poll(() => searches.length).toBe(0);
});

test('a search that found nothing is kept once the user leaves it', async ({ page }) => {
	const { searches } = await serveLibrary(page);
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	const songs = page
		.getByRole('navigation', { name: 'Library' })
		.first()
		.getByRole('link', { name: 'Songs' });
	await page.goto('/albums');

	// Left at once, it was only typing.
	await box.pressSequentially('zzz');
	await expect(page.getByRole('heading', { name: /No music matches/ })).toBeVisible();
	await songs.click();
	await expect(page).toHaveURL(/\/songs$/);
	expect(searches).toEqual([]);

	// Left after it has stood, it was a search, though it found nothing.
	await box.pressSequentially('zzz');
	await expect(page.getByRole('heading', { name: /No music matches/ })).toBeVisible();
	await page.waitForTimeout(2_100);
	await songs.click();
	await expect(page).toHaveURL(/\/songs$/);
	await expect.poll(() => searches.map((s) => [s.query, s.selected])).toEqual([['zzz', null]]);
});

test('a new server sets up its owner, then opens the library', async ({ page }) => {
	await serveLibrary(page, { setupRequired: true });
	await page.goto('/albums');
	await expect(page).toHaveURL(/\/setup$/);
	await expect(page.getByRole('navigation', { name: 'Library' })).toHaveCount(0);

	await page.getByLabel('Username').fill('sam');
	await page.getByLabel('Password', { exact: true }).fill('hunter2');
	await page.getByLabel('Confirm password').fill('hunter3');
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page.getByRole('alert')).toHaveText("The passwords don't match.");

	await page.getByLabel('Confirm password').fill('hunter2');
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page.getByRole('alert')).toHaveText('This is a very common password.');

	await page.getByLabel('Password', { exact: true }).fill('correct horse battery staple');
	await page.getByLabel('Confirm password').fill('correct horse battery staple');
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page).toHaveURL(/\/albums$/);
	await expect(page.getByRole('heading', { level: 1, name: 'Albums' })).toBeVisible();
});

test('setup is not offered once it is done', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/setup');
	await expect(page).toHaveURL(/\/albums$/);
});

test('logging in returns to the page that asked for it', async ({ page }) => {
	await serveLibrary(page, { signedIn: false });
	await page.goto('/artists');
	await expect(page).toHaveURL(/\/login\?next=%2Fartists$/);
	await expect(page.getByRole('navigation', { name: 'Library' })).toHaveCount(0);

	await page.getByLabel('Username').fill('sam');
	await page.getByLabel('Password').fill('wrong');
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page.getByRole('alert')).toHaveText('The username or password is wrong.');

	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page).toHaveURL(/\/artists$/);
	await expect(page.getByRole('heading', { level: 1, name: 'Artists' })).toBeVisible();
});

test('logging out ends the session', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('navigation', { name: 'Library' })
		.getByRole('link', { name: 'Sam' })
		.click();
	await expect(
		page.getByText('The admins of this server can see your listening data')
	).toBeVisible();

	await page.getByRole('button', { name: 'Log out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.goto('/albums');
	await expect(page).toHaveURL(/\/login\?next=%2Falbums$/);
});

test('the account is reachable on a phone', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await serveLibrary(page);
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Account: Sam' }).click();
	await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
});

test('lyrics follow the playing track', async ({ page }) => {
	await serveLibrary(page);
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();
	await page.getByRole('button', { name: 'Signal Part 1' }).click();

	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: 'Lyrics' }).click();
	const panel = page.getByRole('complementary', { name: 'Lyrics' });
	await expect(panel.getByRole('button', { name: 'First line of the song' })).toBeVisible();
	// The fixture's audio is two seconds long, so the second line comes up.
	await expect(panel.getByRole('button', { name: 'Second line of the song' })).toHaveAttribute(
		'aria-current',
		'true'
	);

	await player.getByRole('button', { name: 'Next' }).click();
	await expect(panel.getByText('No lyrics for this song.')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(panel).toBeHidden();
});

test('a user connects a plugin with their own token, and can disconnect it', async ({ page }) => {
	await serveLibrary(page, { role: 'user' });
	await page.goto('/account');
	const connections = page.getByRole('region', { name: 'Connections' });
	await connections.getByRole('button', { name: 'Connect…' }).click();

	const dialog = page.getByRole('dialog', { name: 'Connect Scrobbler' });
	await dialog.getByLabel('User token').fill('wrong-token');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog.getByRole('alert')).toHaveText('The scrobbler refused this token.');
	await dialog.getByLabel('User token').fill('good-token');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toBeHidden();
	await expect(connections.getByText('Connected')).toBeVisible();

	await connections.getByRole('button', { name: 'Disconnect' }).click();
	await expect(connections.getByRole('button', { name: 'Connect…' })).toBeVisible();
});
