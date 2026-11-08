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
async function serveCatalog(page: Page, asked: string[]) {
	await page.route('**/api/v1/**', async (route) => {
		const path = new URL(route.request().url()).pathname.replace(/^.*\/api\/v1/, '');
		asked.push(path);
		const feed = { json: { changes: [], next: 0, head: 0 } };
		if (path === '/libraries')
			return route.fulfill({ json: { items: [library, { ...library, id: '1' }] } });
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
		if (path === '/libraries/1/changes' || path === '/me/changes') return route.fulfill(feed);
		return route.fallback();
	});
}

/** Asks the catalog's owner for a library's tracks as the app's other code will: over the channel. */
function listTracks(page: Page) {
	return page.evaluate(
		() =>
			new Promise<{ items: { title: string }[] }>((resolve, reject) => {
				const channel = new BroadcastChannel('jewelcase-catalog');
				const id = crypto.randomUUID();
				const send = () =>
					channel.postMessage({
						type: 'request',
						id,
						request: { type: 'query', library: '1', name: 'listTracks', params: {} }
					});
				channel.onmessage = ({ data }) => {
					if (data.id !== id) return;
					channel.close();
					if (data.error) reject(new Error(data.error));
					else resolve(JSON.parse(data.result));
				};
				// Until the owner has opened the files nobody answers; an owner says so when it does.
				const timer = setInterval(send, 250);
				setTimeout(() => {
					clearInterval(timer);
					channel.close();
					reject(new Error('no catalog owner answered'));
				}, 20_000);
				send();
			})
	);
}

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
	const fromFirst = await titles(first);
	expect(fromFirst.length).toBeGreaterThan(0);
	expect(await titles(second)).toEqual(fromFirst);

	// The first page owns the database; closing it hands the files to the second, which opens them
	// without downloading anything again.
	await first.close();
	expect(await titles(second)).toEqual(fromFirst);
	expect(asked.filter((p) => p === '/libraries/1/catalog')).toHaveLength(1);
});
