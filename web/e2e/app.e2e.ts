import { type Server, setupOwner } from './server';
import { expect, login, test } from './test';

test.describe('on a server of its own, logged out', () => {
	test.use({ loggedIn: false });

	test('explains an empty server instead of erroring', async ({ page, freshServer }) => {
		const { url } = await freshServer({ music: false });
		const { username, password } = await setupOwner(url);
		await login(page, url, username, password);
		await page.goto(`${url}/`);
		await expect(page.getByRole('heading', { name: 'No music yet' })).toBeVisible();
	});

	test('a new server sets up its owner, then opens the library', async ({ page, freshServer }) => {
		const { url } = await freshServer();
		await page.goto(`${url}/albums`);
		await expect(page).toHaveURL(/\/setup$/);
		await expect(page.getByRole('navigation', { name: 'Library' })).toHaveCount(0);

		await page.getByLabel('Username').fill('sam');
		await page.getByLabel('Password', { exact: true }).fill('hunter2');
		await page.getByLabel('Confirm password').fill('hunter3');
		await page.getByRole('button', { name: 'Create account' }).click();
		await expect(page.getByRole('alert')).toHaveText("The passwords don't match.");

		await page.getByLabel('Confirm password').fill('hunter2');
		await page.getByRole('button', { name: 'Create account' }).click();
		await expect(page.getByRole('alert')).toHaveText(/This is a very common password\./);

		const strong = `e2e-${crypto.randomUUID()}`;
		await page.getByLabel('Password', { exact: true }).fill(strong);
		await page.getByLabel('Confirm password').fill(strong);
		await page.getByRole('button', { name: 'Create account' }).click();
		await expect(page).toHaveURL(/\/albums$/);
		await expect(page.getByRole('heading', { level: 1, name: 'Albums' })).toBeVisible();
	});

	test('logging in returns to the page that asked for it', async ({ page, server }) => {
		await page.goto('/artists');
		await expect(page).toHaveURL(/\/login\?next=%2Fartists$/);
		await expect(page.getByRole('navigation', { name: 'Library' })).toHaveCount(0);

		await page.getByLabel('Username').fill(server.username);
		await page.getByLabel('Password').fill('wrong');
		await page.getByRole('button', { name: 'Log in' }).click();
		await expect(page.getByRole('alert')).toHaveText('The username or password is wrong.');

		await page.getByLabel('Password').fill(server.password);
		await page.getByRole('button', { name: 'Log in' }).click();
		await expect(page).toHaveURL(/\/artists$/);
		await expect(page.getByRole('heading', { level: 1, name: 'Artists' })).toBeVisible();
	});
});

/** The query and chosen result of each recent search on the server, newest first. */
const recentSearches = (server: Server) => async () =>
	(await server.api(`/libraries/${server.ids.library}/recent-searches`)).items.map(
		(s: { query: string; selected: unknown }) => [s.query, s.selected]
	);

test('sidebar sections route to their views', async ({ page }) => {
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

test('opening an album and clicking a song plays it', async ({ page, server }) => {
	const plays: { playId: string; trackId: string; end: string | null }[] = [];
	page.on('request', (r) => {
		if (r.method() === 'POST' && r.url().endsWith('/api/v1/me/plays')) {
			plays.push(...r.postDataJSON().items);
		}
	});
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();

	await expect(page.getByRole('heading', { level: 1, name: 'Signal' })).toBeVisible();
	await expect(page.getByText('Disc 1')).toBeVisible();
	await expect(page.getByText('Disc 2')).toBeVisible();

	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await expect(player.getByRole('button', { name: 'Pause' })).toBeVisible();

	// The song's own start is reported once its audio plays.
	await expect.poll(() => plays.length).toBeGreaterThan(0);
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();

	// A play is reported as it starts and again as it ends, with the album it was played from.
	const ends = () => plays.filter((p) => p.playId === plays[0]?.playId).map((p) => p.end);
	await expect.poll(ends).toEqual([null, 'skipped']);
	expect(plays[0]).toMatchObject({
		trackId: server.ids.tracks['Signal Part 2'],
		context: { type: 'album' },
		origin: 'context'
	});
});

test('search updates as the user types', async ({ page }) => {
	await page.goto('/albums');
	await page
		.getByRole('searchbox', { name: 'Search the library' })
		.first()
		.pressSequentially('remix');

	await expect(page).toHaveURL(/\/search\?q=remix$/);
	await expect(page.getByRole('button', { name: 'Signal Remix', exact: true })).toBeVisible();

	await page.getByRole('searchbox', { name: 'Search the library' }).first().fill('zzz');
	await expect(page.getByRole('heading', { name: /No music matches/ })).toBeVisible();
});

test('a search played from is offered again when the box is empty, and can be removed', async ({
	page,
	server
}) => {
	const searches = recentSearches(server);
	await page.goto('/albums');
	const box = page.getByRole('searchbox', { name: 'Search the library' }).first();
	await box.pressSequentially('remix');
	await page.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await expect
		.poll(searches)
		.toEqual([['remix', { type: 'track', id: server.ids.tracks['Signal Remix'] }]]);

	await box.fill('');
	const recent = page.getByRole('region', { name: 'Recent searches' });
	await recent.getByRole('button', { name: 'remix', exact: true }).click();
	await expect(page).toHaveURL(/\/search\?q=remix$/);

	await box.fill('');
	await recent.getByRole('button', { name: 'Remove “remix” from recent searches' }).click();
	await expect(recent).toHaveCount(0);
	await expect.poll(searches).toEqual([]);
});

test('a search that found nothing is kept once the user leaves it', async ({ page, server }) => {
	const searches = recentSearches(server);
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
	expect(await searches()).toEqual([]);

	// Left after it has stood, it was a search, though it found nothing.
	await box.pressSequentially('zzz');
	await expect(page.getByRole('heading', { name: /No music matches/ })).toBeVisible();
	await page.waitForTimeout(2_100);
	await songs.click();
	await expect(page).toHaveURL(/\/songs$/);
	await expect.poll(searches).toEqual([['zzz', null]]);
});

test('setup is not offered once it is done', async ({ page }) => {
	await page.goto('/setup');
	await expect(page).toHaveURL(/\/albums$/);
});

test('logging out ends the session', async ({ page }) => {
	await page.goto('/albums');
	await page
		.getByRole('navigation', { name: 'Library' })
		.getByRole('link', { name: 'owner' })
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
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Account: owner' }).click();
	await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible();
});

test('lyrics follow the playing track', async ({ page }) => {
	await page.goto('/albums');
	await page
		.getByRole('link', { name: /Signal/ })
		.first()
		.click();
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();

	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: 'Lyrics' }).click();
	const panel = page.getByRole('complementary', { name: 'Lyrics' });
	await expect(panel.getByRole('button', { name: 'First line of the song' })).toBeVisible();
	// A second into the song, the second line comes up.
	await expect(panel.getByRole('button', { name: 'Second line of the song' })).toHaveAttribute(
		'aria-current',
		'true'
	);

	await player.getByRole('button', { name: 'Next' }).click();
	await expect(panel.getByText('No lyrics for this song.')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(panel).toBeHidden();
});
