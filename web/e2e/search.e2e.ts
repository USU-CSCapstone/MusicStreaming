// A search's top result, its tabs, and a tab's every match, against real servers
// (`requirements/search.md` §4–5).

import { execFile } from 'node:child_process';
import { join } from 'node:path';
import { promisify } from 'node:util';
import type { Page } from '@playwright/test';
import { setupOwner, type Server } from './server';
import { createPlaylist, expect, login, test } from './test';

const run = promisify(execFile);

/** The page's main content. */
const main = (page: Page) => page.locator('main');
/** The player bar. */
const player = (page: Page) => page.getByRole('region', { name: 'Player' });
/** The tabs above a search's results. */
const tabs = (page: Page) => page.getByRole('navigation', { name: 'Search results' });
/** The titles of the songs listed in the page's main content. */
const songTitles = (page: Page) => main(page).locator('.track .title');
/** The search box, which at desktop width is the sidebar's. */
const searchBox = (page: Page) =>
	page.getByRole('searchbox', { name: 'Search the library' }).first();

/** The query and chosen result of each recent search on the server, newest first. */
const recentSearches = (server: Server) => async () =>
	(await server.api(`/libraries/${server.ids.library}/recent-searches`)).items.map(
		(s: { query: string; selected: unknown }) => [s.query, s.selected]
	);

test('the top result plays a song or an album from where it stands, and keeps the search', async ({
	page,
	server
}) => {
	const searches = recentSearches(server);
	await page.goto('/search?q=signal%20remix');
	const top = page.getByRole('region', { name: 'Top result' });
	await expect(top.getByText('Song', { exact: true })).toBeVisible();
	await expect(top.getByText('Signal Remix')).toBeVisible();
	// It is a shortcut, so the song is in its section too.
	await expect(songTitles(page).first()).toHaveText('Signal Remix');

	await top.getByRole('button', { name: 'Play Signal Remix' }).click();
	await expect(player(page).getByText('Signal Remix')).toBeVisible();
	await expect
		.poll(searches)
		.toEqual([['signal remix', { type: 'track', id: server.ids.tracks['Signal Remix'] }]]);

	await page.goto('/search?q=hearth');
	await expect(top.getByText('Album', { exact: true })).toBeVisible();
	await top.getByRole('button', { name: 'Play Hearth' }).click();
	// The album plays from its first song, as its page's Play does.
	await expect(player(page).getByText('Kindling')).toBeVisible();
	await expect
		.poll(async () => (await searches())[0])
		.toEqual(['hearth', { type: 'album', id: server.ids.albums.Hearth }]);
});

test('a tab shows one type of result, and stays as the search is typed', async ({ page }) => {
	await page.goto('/search?q=signal');
	await expect(tabs(page).getByRole('link', { name: 'All' })).toHaveAttribute(
		'aria-current',
		'page'
	);
	await expect(page.getByRole('region', { name: 'Top result' })).toBeVisible();
	await expect(main(page).getByRole('heading', { name: 'Albums' })).toBeVisible();

	await tabs(page).getByRole('link', { name: 'Songs' }).click();
	await expect(page).toHaveURL(/\/search\?q=signal&type=tracks$/);
	await expect(tabs(page).getByRole('link', { name: 'Songs' })).toHaveAttribute(
		'aria-current',
		'page'
	);
	await expect(songTitles(page)).toHaveCount(3);
	// One type at a time, with no top result.
	await expect(page.getByRole('region', { name: 'Top result' })).toHaveCount(0);
	await expect(main(page).getByRole('heading', { name: 'Albums' })).toHaveCount(0);

	await searchBox(page).fill('kindling');
	await expect(page).toHaveURL(/\/search\?q=kindling&type=tracks$/);
	await expect(songTitles(page)).toHaveText(['Kindling']);

	await tabs(page).getByRole('link', { name: 'Playlists' }).click();
	await expect(
		main(page).getByRole('heading', { name: 'No playlists match “kindling”' })
	).toBeVisible();
	await page.goBack();
	await expect(page).toHaveURL(/type=tracks$/);
	await expect(songTitles(page)).toHaveText(['Kindling']);
});

test('a genre or a mood is found by its name, in a section and a tab of its type', async ({
	page,
	server
}) => {
	const searches = recentSearches(server);
	await page.goto('/search?q=ambient');
	const top = page.getByRole('region', { name: 'Top result' });
	await expect(top.getByText('Genre', { exact: true })).toBeVisible();
	await expect(top.getByRole('link', { name: 'Ambient' })).toBeVisible();
	// A tab for each type of tag the library has, after the rest, as the sidebar lists them.
	await expect(tabs(page).getByRole('link')).toHaveText([
		'All',
		'Albums',
		'Artists',
		'Playlists',
		'Songs',
		'Genres',
		'Moods'
	]);

	// Played, the genre plays its songs album by album, and the search is kept, naming nothing.
	await top.getByRole('button', { name: 'Play Ambient' }).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	await expect.poll(searches).toEqual([['ambient', null]]);

	await main(page).getByRole('heading', { name: 'Genres' }).getByRole('link').click();
	await expect(page).toHaveURL(/\/search\?q=ambient&type=tags&tagType=genre$/);
	await expect(main(page).getByRole('link', { name: /^Ambient/ })).toBeVisible();
	await searchBox(page).fill('calm');
	await expect(page).toHaveURL(/q=calm&type=tags&tagType=genre$/);
	await expect(main(page).getByRole('heading', { name: 'No genres match “calm”' })).toBeVisible();
	await tabs(page).getByRole('link', { name: 'Moods' }).click();
	await expect(page).toHaveURL(/type=tags&tagType=mood$/);
	await main(page).getByRole('link', { name: /^Calm/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Calm' })).toBeVisible();
});

test('a genre every song of which is hidden is not found', async ({ page, server }) => {
	// Folk is on Kindling alone.
	const { library, tracks } = server.ids;
	await server.api(`/libraries/${library}/tracks/${tracks.Kindling}/hidden`, { method: 'PUT' });
	await page.goto('/search?q=folk');
	await expect(main(page).getByRole('heading', { name: 'No music matches “folk”' })).toBeVisible();
	await page.goto('/search?q=ambient');
	await expect(main(page).getByRole('heading', { name: 'Genres' })).toBeVisible();
});

test('the caller’s playlists are found by their titles, and play from where they stand', async ({
	page,
	server
}) => {
	await createPlaylist(server, 'Road Trip', ['Kindling', 'Signal Remix']);
	await page.goto('/search?q=road%20trip');
	const top = page.getByRole('region', { name: 'Top result' });
	await expect(top.getByText('Playlist', { exact: true })).toBeVisible();
	await top.getByRole('button', { name: 'Play Road Trip' }).click();
	await expect(player(page).getByText('Kindling')).toBeVisible();

	await tabs(page).getByRole('link', { name: 'Playlists' }).click();
	await expect(page).toHaveURL(/type=playlists$/);
	await main(page).getByRole('link', { name: /^Road Trip/ }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Road Trip' })).toBeVisible();
});

test.describe('on a server of its own, with many songs', () => {
	test.use({ loggedIn: false });

	test('Show all leads to the Songs tab, which holds every song found a page at a time, in its sort', async ({
		page,
		freshServer
	}) => {
		// 120 songs called Lullaby and a number, beside the one the server starts with. They are
		// written a dozen at a time without blocking, so the connections kept open to the server
		// notice it closing them while idle.
		const { url, home } = await freshServer();
		const { username, password, api } = await setupOwner(url);
		const write = (n: number) => {
			const number = String(n).padStart(3, '0');
			const args = ['-v', 'error', '-f', 'lavfi', '-i', 'anullsrc=r=8000:cl=mono', '-t', '1'];
			const tags = { title: `Lullaby ${number}`, artist: 'Night Owl', album: 'Lullabies' };
			for (const [key, value] of Object.entries(tags)) args.push('-metadata', `${key}=${value}`);
			return run('ffmpeg', [
				...args,
				'-c:a',
				'flac',
				join(home, 'music', `lullaby-${number}.flac`)
			]);
		};
		for (let first = 1; first <= 120; first += 12) {
			await Promise.all(Array.from({ length: 12 }, (_, i) => write(first + i)));
		}
		const library = (await api('/libraries')).items[0].id;
		await expect
			.poll(async () => (await api(`/libraries/${library}`)).trackCount, { timeout: 30_000 })
			.toBe(121);
		// The search index catches up with the scan once a search notices it.
		await expect
			.poll(async () => (await api(`/libraries/${library}/search/tracks?q=lullaby&limit=1`)).total)
			.toBe(120);
		await login(page, url, username, password);

		await page.goto(`${url}/search?q=lullaby`);
		// All shows the first few, and leads to the rest.
		await expect(songTitles(page)).toHaveCount(12);
		await main(page).getByRole('link', { name: 'Show all 120' }).click();
		await expect(page).toHaveURL(/type=tracks$/);
		await expect(songTitles(page)).toHaveCount(100);
		await songTitles(page).last().scrollIntoViewIfNeeded();
		await expect(songTitles(page)).toHaveCount(120);
		const ranked = (await songTitles(page).allInnerTexts()).map((t) => t.trim());
		expect(new Set(ranked).size).toBe(120);

		// Sorted, the server sorts every song found, not those loaded.
		await page.goto(`${url}/search?q=lullaby&type=tracks&sort=title&order=desc`);
		await expect(songTitles(page).first()).toHaveText('Lullaby 120');
		await songTitles(page).last().scrollIntoViewIfNeeded();
		await expect(songTitles(page)).toHaveCount(120);
		await expect(songTitles(page).last()).toHaveText('Lullaby 001');
	});
});
