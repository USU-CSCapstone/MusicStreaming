import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import { library, serveLibrary, serveRealtime } from './fixtures';
import goldenRows from '../../crates/catalog/parity/library.json' with { type: 'json' };

const snapshot = readFileSync(new URL('../../crates/catalog/parity/library.db', import.meta.url));
const etag = createHash('sha256').update(snapshot).digest('hex');
/** The schema version the golden file was built at: its SQLite header's `user_version`. */
const schema = String(snapshot.readUInt32BE(60));

/** The server's run, which the feeds and the snapshot are given in. */
const EPOCH = '1';

/** The dev mock has no feeds or snapshot, so serve the golden library's, as library 1, after `serveLibrary`'s routes, which come first otherwise. */
async function serveCatalog(
	page: Page,
	asked: string[],
	ids = ['11', '1'],
	feed: { changes: object[]; next: number; head: number } = { changes: [], next: 0, head: 0 },
	account: { position: number }[] = []
) {
	await page.route('**/api/v1/**', async (route) => {
		const path = new URL(route.request().url()).pathname.replace(/^.*\/api\/v1/, '');
		asked.push(path);
		if (path === '/libraries')
			return route.fulfill({
				json: { items: ids.map((id) => (id === library.id ? library : { ...library, id })) }
			});
		if (path === '/libraries/1/catalog')
			return route.fulfill({
				body: snapshot,
				headers: {
					etag: `"${etag}"`,
					'x-jewelcase-position': '0',
					'x-jewelcase-epoch': EPOCH,
					'x-jewelcase-schema': schema,
					'content-type': 'application/vnd.sqlite3'
				}
			});
		if (path === '/libraries/1/changes') return route.fulfill({ json: { ...feed, epoch: EPOCH } });
		if (path === '/me/changes') {
			const after = Number(new URL(route.request().url()).searchParams.get('after'));
			const head = account.at(-1)?.position ?? 0;
			return route.fulfill({
				json: {
					changes: account.filter(({ position }) => position > after),
					next: head,
					head,
					epoch: EPOCH
				}
			});
		}
		return route.fallback();
	});
}

/** Asks the catalog's owner for `request` as the app's other code will: over the channel. */
function ask(page: Page, request: object) {
	return page.evaluate(
		(request) =>
			new Promise<string>((resolve, reject) => {
				const channel = new BroadcastChannel('jewelcase-catalog');
				const id = crypto.randomUUID();
				const send = () =>
					channel.postMessage({
						type: 'request',
						id,
						request
					});
				channel.onmessage = ({ data }) => {
					if (data.id !== id) return;
					clearInterval(timer);
					channel.close();
					if (data.error) reject(new Error(data.error));
					else resolve(data.result);
				};
				// Until the owner has opened the files nobody answers; an owner says so when it does.
				const timer = setInterval(send, 250);
				setTimeout(() => {
					clearInterval(timer);
					channel.close();
					reject(new Error('no catalog owner answered'));
				}, 20_000);
				send();
			}),
		request
	);
}

/** Library 1's answer to the query `name`, from the catalog's owner. */
const query = async <T>(page: Page, name: string, params: object = {}): Promise<T> =>
	JSON.parse(await ask(page, { type: 'query', library: '1', name, params }));

/** Library 1's first page of tracks, from the catalog's owner. */
const listTracks = (page: Page) => query<{ items: { title: string }[] }>(page, 'listTracks');

test('two pages answer catalog queries with one database', async ({ context }) => {
	const asked: string[] = [];
	/** Opens a page on the library with the golden catalog served, and waits for it to load. */
	const open = async () => {
		const page = await context.newPage();
		await serveLibrary(page);
		await serveRealtime(page);
		await serveCatalog(page, asked);
		await page.goto('/albums');
		return page;
	};
	const first = await open();
	await expect.poll(() => asked.filter((p) => p === '/libraries/1/catalog').length).toBe(1);
	const second = await open();

	/** The titles of the tracks `page` gets from the catalog's owner. */
	const titles = async (page: Page) => (await listTracks(page)).items.map((t) => t.title);
	await synced(first);
	const fromFirst = await titles(first);
	expect(fromFirst.length).toBeGreaterThan(0);
	expect(await titles(second)).toEqual(fromFirst);

	// The first page owns the database; closing it hands the files to the second, which opens them
	// without downloading anything again.
	await first.close();
	expect(await titles(second)).toEqual(fromFirst);
	expect(asked.filter((p) => p === '/libraries/1/catalog')).toHaveLength(1);
});

/** A server with library 1 alone, the golden one, and the realtime connection saying `hint` of it. Returns a count of the realtime connections open. */
async function serveGolden(
	page: Page,
	asked: string[],
	hint: boolean | 'account' = false,
	feed?: Parameters<typeof serveCatalog>[3],
	account?: Parameters<typeof serveCatalog>[4]
) {
	await serveLibrary(page);
	await serveCatalog(page, asked, ['1'], feed, account);
	const sockets = { open: 0 };
	await page.routeWebSocket('**/api/v1/realtime', (ws) => {
		sockets.open++;
		let closed = false;
		const now = () => new Date().toISOString();
		ws.send(JSON.stringify({ type: 'hello', deviceId: 'd1', serverTime: now() }));
		// Said again and again, since a hint heard before the catalog's worker is up is lost.
		const timer = hint
			? setInterval(
					() =>
						ws.send(
							JSON.stringify(
								hint === 'account'
									? { type: 'accountChanged', position: 99 }
									: { type: 'libraryChanged', libraryId: '1', position: 5 }
							)
						),
					300
				)
			: undefined;
		ws.onClose(() => {
			if (!closed) sockets.open--;
			closed = true;
			clearInterval(timer);
		});
		ws.onMessage((data) => {
			if (JSON.parse(data.toString()).type === 'ping')
				ws.send(JSON.stringify({ type: 'pong', serverTime: now() }));
		});
	});
	return sockets;
}

/** Waits until library 1's snapshot is on the device. */
const synced = (page: Page) =>
	expect.poll(() => ask(page, { type: 'onDevice', library: '1' })).toBe('1');

type Albums = { items: { id: string; title: string }[]; total: number };

test('a synced device renders albums with the API blocked', async ({ page }) => {
	const asked: string[] = [];
	await serveGolden(page, asked);
	await page.goto('/');
	await synced(page);
	const albums = await query<Albums>(page, 'listAlbums');
	expect(albums.items.length).toBeGreaterThan(0);

	const blocked: string[] = [];
	await page.route('**/api/v1/**', (route) => {
		blocked.push(new URL(route.request().url()).pathname);
		return route.abort();
	});
	await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
	await expect(page).toHaveURL(/\/albums$/);
	for (const { title } of albums.items.slice(0, 3))
		await expect(page.getByText(title, { exact: true }).first()).toBeVisible();
	expect(blocked.filter((p) => p.endsWith('/albums'))).toEqual([]);
});

test('a never-synced device reads from the server', async ({ page }) => {
	const requests: string[] = [];
	page.on('request', (request) => requests.push(new URL(request.url()).pathname));
	await serveLibrary(page);
	await serveRealtime(page);
	await page.goto('/albums');
	await expect(page.getByText('Signal', { exact: true }).first()).toBeVisible();
	expect(requests.some((p) => p.endsWith('/libraries/11/albums'))).toBe(true);
});

/** Goes to `path` inside the app, as a link click does, so the page loads with the catalog already open. */
const go = (page: Page, path: string) =>
	page.evaluate((path) => {
		const link = document.createElement('a');
		link.href = path;
		document.body.append(link);
		link.click();
		link.remove();
	}, path);

/** A genre tag's upsert at `position`, as the library feed carries it. */
const genre = (position: number, id: string, name: string) => ({
	position,
	op: 'upsert',
	kind: 'tag',
	id,
	rows: {
		tags: [
			{
				id,
				library_id: '1',
				type: 'genre',
				name,
				sort_key: btoa(name),
				track_count: 0,
				album_count: 0,
				artist_count: 0
			}
		],
		tag_types: [{ key: 'genre', name: 'Genre' }]
	}
});

test('a stale device shows local rows, then the server’s', async ({ page }) => {
	const asked: string[] = [];
	const feed = { changes: [] as object[], next: 0, head: 0 };
	// The server hints a feed position that its feed does not yet reach, so the device stays behind.
	await serveGolden(page, asked, true, feed);
	await page.goto('/');
	await synced(page);
	const local = await query<{ items: { name: string }[] }>(page, 'listTags', { type: 'genre' });

	// Behind, the page shows the device's rows without asking the server.
	await go(page, '/tags?type=genre');
	await expect(page.getByText(local.items[0].name, { exact: true }).first()).toBeVisible();
	await expect(page.getByText('Funk', { exact: true })).toHaveCount(0);
	expect(asked.filter((p) => p.endsWith('/libraries/1/tags'))).toEqual([]);

	// The pull brings the server's change in; the page shows it when it loads again.
	feed.changes = [genre(5, '90', 'Funk')];
	feed.next = feed.head = 5;
	await expect
		.poll(async () =>
			(await query<{ items: { name: string }[] }>(page, 'listTags', { type: 'genre' })).items.some(
				(tag) => tag.name === 'Funk'
			)
		)
		.toBe(true);
	await go(page, '/albums');
	await expect(page).toHaveURL(/\/albums$/);
	await go(page, '/tags?type=genre');
	await expect(page.getByText('Funk', { exact: true })).toBeVisible();
	expect(asked.filter((p) => p.endsWith('/libraries/1/tags'))).toEqual([]);
});

test('an offline device that never synced says the library is not on it yet', async ({ page }) => {
	await serveLibrary(page);
	await serveRealtime(page);
	// No snapshot to download, so the library never reaches the device.
	await page.route('**/api/v1/libraries/*/catalog', (route) => route.fulfill({ status: 404 }));
	await page.goto('/');
	await expect(page.getByRole('link', { name: 'Artists', exact: true }).first()).toBeVisible();
	// Only the browser's word that it is offline: pages still load, which a real outage would stop.
	await page.evaluate(() => Object.defineProperty(navigator, 'onLine', { get: () => false }));
	await page.getByRole('link', { name: 'Artists', exact: true }).first().click();
	await expect(page.getByRole('heading', { name: 'Not on this device yet' })).toBeVisible();
});

/** A one pixel picture, for the images the worker keeps. */
const PIXEL = Buffer.from('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7', 'base64');

/** An album's upsert at `position`, as the library feed carries it. */
const album = (position: number, id: string, title: string) => ({
	position,
	op: 'upsert',
	kind: 'album',
	id,
	rows: {
		albums: [
			{
				added_at: 5000,
				artist_sort_key: '',
				available_track_count: 0,
				disc_count: 1,
				disc_total: null,
				duration_us: 0,
				id,
				image_id: null,
				labels: '[]',
				library_id: '1',
				loudness_lufs: null,
				peak_dbtp: null,
				release_date: null,
				sort_key: btoa(title.toLowerCase()),
				title,
				track_count: 0,
				track_total: null,
				type: 'album'
			}
		]
	}
});

test('a libraryChanged hint adds the new album to the open albums page without losing scroll', async ({
	page
}) => {
	// Short enough that four albums need scrolling.
	await page.setViewportSize({ width: 600, height: 240 });
	const asked: string[] = [];
	const feed = { changes: [] as object[], next: 0, head: 0 };
	await serveGolden(page, asked, true, feed);
	await page.goto('/');
	await synced(page);
	await go(page, '/albums');
	const first = page.getByText('Abbey Road', { exact: true }).first();
	await expect(first).toBeAttached();
	const main = page.locator('main');
	await first.evaluate((el) => (el.dataset.kept = 'yes'));
	await main.evaluate((m) => (m.scrollTop = m.scrollHeight));
	const before = await main.evaluate((m) => m.scrollTop);
	expect(before).toBeGreaterThan(0);

	feed.changes = [album(5, '105', 'Aardvark')];
	feed.next = feed.head = 5;
	await expect(page.getByText('Aardvark', { exact: true }).first()).toBeAttached();
	// The last album, which the new one pushed past the loaded window, is loaded in turn.
	await expect(page.getByText('Nevermind', { exact: true }).first()).toBeAttached();
	// The page was not remounted, and the user is not at the top.
	await expect(first).toHaveAttribute('data-kept', 'yes');
	expect(await main.evaluate((m) => m.scrollTop)).toBeGreaterThanOrEqual(before);
});

test('a playlist edited on another device updates the open playlist', async ({ page }) => {
	const asked: string[] = [];
	const rows = (goldenRows as { kind: string; id: string; key: number; tables: object }[])
		.filter(({ kind }) => kind === 'playlist' || kind === 'playlistItem')
		.map(({ kind, id, key, tables }, i) => ({
			position: i + 1,
			op: 'upsert',
			kind,
			id,
			key,
			rows: tables
		}));
	const account = [...rows];
	await serveGolden(page, asked, 'account', undefined, account);
	await page.goto('/');
	await synced(page);
	await go(page, '/playlists/501');
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();

	// Another device renamed it; the account feed carries that to this one.
	const renamed = structuredClone(rows.find(({ kind }) => kind === 'playlist')!) as {
		position: number;
		rows: { playlists: { title: string; version: number }[] };
	};
	renamed.position = account.length + 1;
	renamed.rows.playlists[0].title = 'Renamed Elsewhere';
	renamed.rows.playlists[0].version = 4;
	account.push(renamed);
	await expect(page.getByRole('heading', { name: 'Renamed Elsewhere' })).toBeVisible();
});

test('searching offline finds a synced track', async ({ page }) => {
	const asked: string[] = [];
	await serveGolden(page, asked);
	await page.goto('/');
	await synced(page);
	await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
	await expect(page).toHaveURL(/\/albums$/);

	const blocked: string[] = [];
	await page.route('**/api/v1/**', (route) => {
		blocked.push(new URL(route.request().url()).pathname);
		return route.abort();
	});
	await page.evaluate(() => Object.defineProperty(navigator, 'onLine', { get: () => false }));
	await page.getByRole('searchbox', { name: 'Search the library' }).fill('come tog');
	await expect(page).toHaveURL(/\/search\?q=come/);
	await expect(page.getByText('Come Together').first()).toBeVisible();
	expect(blocked.filter((p) => p.endsWith('/search'))).toEqual([]);
});

test('starting an album offline queues all its tracks', async ({ page }) => {
	const asked: string[] = [];
	await serveGolden(page, asked);
	await page.goto('/');
	await synced(page);
	// The album with the most tracks, and its tracks in the order its page shows them.
	const albums = await query<{ items: { id: string; trackCount: number }[] }>(page, 'listAlbums');
	const { id } = albums.items.reduce((most, one) =>
		one.trackCount > most.trackCount ? one : most
	);
	const tracks = await query<{ items: { title: string; availability: string }[] }>(
		page,
		'listTracks',
		{
			albumId: id,
			sort: 'album',
			order: 'asc'
		}
	);
	// A missing track keeps its place, shown as unavailable (D36).
	const titles = tracks.items.map((t) =>
		t.availability === 'missing' ? 'Unavailable track' : t.title
	);
	expect(titles.length).toBeGreaterThan(2);

	await page.route('**/api/v1/**', (route) => route.abort());
	await page.evaluate(() => Object.defineProperty(navigator, 'onLine', { get: () => false }));
	await go(page, `/albums/${id}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: 'Queue' }).click();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	const upNext = () =>
		panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts();
	await expect.poll(upNext).toEqual(titles.slice(1));

	// The album page gave the queue every row it shows, so only the list the device built and
	// kept, while every API call was aborted, tells the two apart.
	await expect
		.poll(() =>
			page.evaluate(async () => (await (await caches.open('jewelcase-queue-lists')).keys()).length)
		)
		.toBe(1);
});

test.describe('the offline shell', () => {
	// The service worker runs here, as in `pwa.e2e.ts`: it answers the reloads below from its own files.
	test.use({ serviceWorkers: 'allow' });

	test('a page not opened before loads offline on the first visit', async ({ page, context }) => {
		await serveGolden(page, []);
		await page.goto('/');
		await synced(page);
		// No reload: the worker takes over the page it was installed from.
		await page.waitForFunction(() => navigator.serviceWorker.controller !== null);
		const { items } = await query<Albums>(page, 'listAlbums');

		await context.setOffline(true);
		await page.route('**/api/v1/**', (route) => route.abort());
		await go(page, `/albums/${items[0].id}`);
		await expect(page.getByRole('heading', { name: items[0].title })).toBeVisible();
	});

	test('reloading offline after one visit shows the app', async ({ page, context }) => {
		await serveGolden(page, []);
		await page.goto('/');
		await synced(page);
		await page.evaluate(() => navigator.serviceWorker.ready);
		const { items } = await query<Albums>(page, 'listAlbums');
		expect(items.length).toBeGreaterThan(0);

		// The stubbed API would answer offline too, so it is closed as a dead network is.
		await context.setOffline(true);
		await page.route('**/api/v1/**', (route) => route.abort());
		await page.reload();
		await expect(page.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.getByText(items[0].title, { exact: true }).first()).toBeVisible();
	});

	test('reloading an album page offline shows the album', async ({ page, context }) => {
		await serveGolden(page, []);
		await page.goto('/');
		await synced(page);
		await page.evaluate(() => navigator.serviceWorker.ready);
		const { items } = await query<Albums>(page, 'listAlbums');
		await go(page, `/albums/${items[0].id}`);
		await expect(page.getByRole('heading', { name: items[0].title })).toBeVisible();

		// Launched at the deep link with no network, as an installed app reopens where it was left.
		await context.setOffline(true);
		await page.route('**/api/v1/**', (route) => route.abort());
		await page.reload();
		await expect(page.getByRole('heading', { name: items[0].title })).toBeVisible();
	});

	test('reloading with the server unreachable shows the app', async ({ page }) => {
		await serveGolden(page, []);
		await page.goto('/');
		await synced(page);
		await page.evaluate(() => navigator.serviceWorker.ready);
		const { items } = await query<Albums>(page, 'listAlbums');

		// The browser still says it is online, as on mobile data away from the server.
		await page.route('**/api/v1/**', (route) => route.abort());
		await page.reload();
		await expect(page.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.getByText(items[0].title, { exact: true }).first()).toBeVisible();
	});

	test('an album seen online shows its art offline', async ({ page, context }) => {
		const withArt = album(5, '105', 'Pictured');
		withArt.rows.albums[0].image_id = '900';
		(withArt.rows as Record<string, object[]>).images = [
			{ id: '900', library_id: '1', width: 1, height: 1, placeholder: '' }
		];
		const feed = { changes: [withArt], next: 5, head: 5 };
		await serveGolden(page, [], true, feed);
		// The worker's own requests bypass the page's routes, so the context answers the images.
		const served: string[] = [];
		await context.route('**/api/v1/libraries/1/images/*', (route) => {
			served.push(route.request().url());
			return route.fulfill({ body: PIXEL, contentType: 'image/gif' });
		});
		await page.goto('/');
		await synced(page);
		await expect
			.poll(async () =>
				(await query<Albums>(page, 'listAlbums')).items.some(({ title }) => title === 'Pictured')
			)
			.toBe(true);
		// The worker takes over pages on their next load. Online, pages read the server until the
		// device's owner serves, and this test's server lacks the album, so wait for the owner.
		await page.evaluate(() => navigator.serviceWorker.ready);
		await page.reload();
		await synced(page);
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.locator('img.loaded').first()).toBeVisible();
		await expect
			.poll(() => page.evaluate(async () => (await (await caches.open('images')).keys()).length))
			.toBeGreaterThan(0);
		await go(page, '/account');
		await expect(page.getByText(/^\d+ images? · /)).toBeVisible();
		await go(page, '/albums');

		await context.setOffline(true);
		await page.route('**/api/v1/**', (route) => route.abort());
		const before = served.length;
		await page.reload();
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.getByText('Pictured', { exact: true }).first()).toBeVisible();
		await expect(page.locator('img.loaded').first()).toBeVisible();
		expect(served).toHaveLength(before);
	});
});

/** Turns Work offline on or off from the account page, where it lives. */
async function workOffline(page: Page, on: boolean) {
	await go(page, '/account');
	const toggle = page.getByRole('switch', { name: 'Work offline' });
	await expect(toggle).toBeVisible();
	await (on ? toggle.check() : toggle.uncheck());
}

test('Work offline stops network requests and the library still browses', async ({
	page,
	context
}) => {
	const sockets = await serveGolden(page, []);
	// Every request of the page and of its workers, and the realtime connections.
	const requests: string[] = [];
	context.on('request', (request) => {
		if (request.url().includes('/api/v1/')) requests.push(new URL(request.url()).pathname);
	});
	const open = () => sockets.open;
	await page.goto('/');
	await synced(page);
	await expect.poll(open).toBe(1);
	const { items } = await query<Albums>(page, 'listAlbums');

	// A track has been played, so the window below starts after audio was requested.
	await go(page, `/albums/${items[0].id}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect.poll(() => requests.some((path) => path.endsWith('/audio'))).toBe(true);

	await workOffline(page, true);
	await expect.poll(open).toBe(0);
	await expect(page.getByRole('region', { name: 'Player' }).getByRole('status')).toHaveText(
		'Working offline'
	);
	// The server's side of the sidebar says it cannot be asked.
	await expect(page.getByText('scan status unavailable offline')).toBeVisible();
	requests.length = 0;

	// Browsing and starting an album read the device; the queue worker's edit has nowhere to go.
	await go(page, '/albums');
	await expect(page.getByText(items[0].title, { exact: true }).first()).toBeVisible();
	await go(page, `/albums/${items[0].id}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	// A fresh start offline reads the device too.
	await page.goto('/');
	await synced(page);
	await go(page, '/albums');
	await expect(page.getByText(items[0].title, { exact: true }).first()).toBeVisible();
	// Past the queue worker's retry wait and a realtime reconnect's first backoff.
	await page.waitForTimeout(3500);
	expect(requests).toEqual([]);
	expect(open()).toBe(0);

	// Turning it off connects again, and the device catches up.
	await workOffline(page, false);
	await expect.poll(open).toBe(1);
	await expect.poll(() => requests.length).toBeGreaterThan(0);
	await expect(page.getByRole('region', { name: 'Player' }).getByRole('status')).toBeHidden();
});

test('admin plugins page says it needs the server when offline', async ({ page }) => {
	await serveLibrary(page);
	await serveRealtime(page);
	await page.goto('/account');
	await page.getByRole('switch', { name: 'Work offline' }).check();
	// A fresh start offline: the account and library list are the ones kept, and the page opens.
	await page.reload();
	await expect(page.getByRole('switch', { name: 'Work offline' })).toBeChecked();
	await expect(page.getByText('Unavailable offline. This needs the server')).toBeVisible();

	// Shown, not hidden: the link stays, and the page says why it has nothing.
	await page.getByRole('link', { name: 'Plugins' }).click();
	await expect(page.getByRole('heading', { name: 'Unavailable offline' })).toBeVisible();
	await expect(page.getByText('needs the server')).toBeVisible();
	await expect(page.getByRole('link', { name: 'Plugins' })).toBeVisible();
});
