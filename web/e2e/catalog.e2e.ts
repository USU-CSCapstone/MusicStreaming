import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import { library, serveLibrary, serveRealtime } from './fixtures';

const snapshot = readFileSync(new URL('../../crates/catalog/parity/library.db', import.meta.url));
const etag = createHash('sha256').update(snapshot).digest('hex');
/** The schema version the golden file was built at: its SQLite header's `user_version`. */
const schema = String(snapshot.readUInt32BE(60));

/** The dev mock has no feeds or snapshot, so serve the golden library's, as library 1, after `serveLibrary`'s routes, which come first otherwise. */
async function serveCatalog(
	page: Page,
	asked: string[],
	ids = ['11', '1'],
	feed: { changes: object[]; next: number; head: number } = { changes: [], next: 0, head: 0 }
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
					'x-jewelcase-seq': '0',
					'x-jewelcase-schema': schema,
					'content-type': 'application/vnd.sqlite3'
				}
			});
		if (path === '/libraries/1/changes') return route.fulfill({ json: feed });
		if (path === '/me/changes') return route.fulfill({ json: { changes: [], next: 0, head: 0 } });
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

/** A server with library 1 alone, the golden one, and the realtime connection saying `hint` of it. */
async function serveGolden(
	page: Page,
	asked: string[],
	hint = false,
	feed?: Parameters<typeof serveCatalog>[3]
) {
	await serveLibrary(page);
	await serveCatalog(page, asked, ['1'], feed);
	await page.routeWebSocket('**/api/v1/realtime', (ws) => {
		const now = () => new Date().toISOString();
		ws.send(JSON.stringify({ type: 'hello', deviceId: 'd1', serverTime: now() }));
		// Said again and again, since a hint heard before the catalog's worker is up is lost.
		const timer = hint
			? setInterval(
					() => ws.send(JSON.stringify({ type: 'libraryChanged', libraryId: '1', seq: 5 })),
					300
				)
			: undefined;
		ws.onClose(() => clearInterval(timer));
		ws.onMessage((data) => {
			if (JSON.parse(data.toString()).type === 'ping')
				ws.send(JSON.stringify({ type: 'pong', serverTime: now() }));
		});
	});
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

/** A genre tag's upsert at `seq`, as the library feed carries it. */
const genre = (seq: number, id: string, name: string) => ({
	seq,
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
