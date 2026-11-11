import { execFileSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import type { BrowserContext, Page } from '@playwright/test';
import { addAdmin, endBrowserSessions } from './server';
import { type Device, createPlaylist, expect, login, test } from './test';

// The device catalog against real servers. Offline is the server stopped, which is what an outage
// looks like to every request of the page and its workers; `context.setOffline` is used with it
// only for `navigator.onLine`.

/**
 * Asks the catalog's owner for `request` as the app's other code will: over the channel, answered
 * on a channel of the asker's own.
 */
function ask(page: Page, request: object) {
	return page.evaluate(
		(request) =>
			new Promise<string>((resolve, reject) => {
				const channel = new BroadcastChannel('jewelcase-catalog');
				const id = crypto.randomUUID();
				const reply = `jewelcase-catalog-reply-${id}`;
				const answers = new BroadcastChannel(reply);
				const send = () =>
					channel.postMessage({ protocol: 2, type: 'request', id, reply, request });
				answers.onmessage = ({ data }) => {
					if (data.id !== id) return;
					clearInterval(timer);
					channel.close();
					answers.close();
					if (data.error) reject(new Error(data.error));
					else resolve(data.result);
				};
				// Until the owner has opened the files nobody answers; an owner says so when it does.
				const timer = setInterval(send, 250);
				setTimeout(() => {
					clearInterval(timer);
					channel.close();
					answers.close();
					reject(new Error('no catalog owner answered'));
				}, 20_000);
				send();
			}),
		request
	);
}

/** `library`'s answer to the query `name`, from the catalog's owner. */
const query = async <T>(page: Page, library: string, name: string, params: object = {}) =>
	JSON.parse(await ask(page, { type: 'query', library, name, params })) as T;

/**
 * What the catalog's owner says it holds: the state it broadcasts when asked, or null if no owner
 * serves.
 */
function ownerState(page: Page) {
	return page.evaluate(
		() =>
			new Promise<{ serving: boolean; libraries: Record<string, { onDevice: boolean }> } | null>(
				(resolve) => {
					const channel = new BroadcastChannel('jewelcase-catalog');
					const done = (state: never) => {
						clearTimeout(timer);
						channel.close();
						resolve(state);
					};
					const timer = setTimeout(() => done(null as never), 1000);
					channel.onmessage = ({ data }) => data.type === 'state' && done(data.state);
					channel.postMessage({ protocol: 2, type: 'who' });
				}
			)
	);
}

/** Waits until `library`'s snapshot is on the device. */
const synced = (page: Page, library: string) =>
	expect
		.poll(async () => (await ownerState(page))?.libraries[library]?.onDevice, { timeout: 30_000 })
		.toBe(true);

/**
 * Opens the app at `/` and waits until the library is on the device and the service worker has
 * kept the app's files, which is what a person has done by the time they go offline.
 */
async function visit(page: Page, library: string) {
	await page.goto('/');
	await synced(page, library);
	await page.evaluate(() => navigator.serviceWorker.ready);
}

/** The part of a `listAlbums` answer the specs read. */
type Albums = { items: { id: string; title: string }[]; total: number };

/**
 * Goes to `path` inside the app, as a link click does, so the page loads with the catalog already
 * open.
 */
const go = (page: Page, path: string) =>
	page.evaluate((path) => {
		const link = document.createElement('a');
		link.href = path;
		document.body.append(link);
		link.click();
		link.remove();
	}, path);

/** The API requests of the page and its workers that failed, as they happen. */
function failures(context: BrowserContext) {
	const failed: string[] = [];
	context.on('requestfailed', (request) => {
		if (request.url().includes('/api/v1/')) failed.push(new URL(request.url()).pathname);
	});
	return failed;
}

/**
 * Starts a dedicated worker that asks the API for something, and checks that its request failed:
 * the device really is cut off for the workers too, as the catalog and the queue run in them.
 */
async function workersAreCutOff(page: Page, failed: string[]) {
	const answer = await page.evaluate(
		(url) =>
			new Promise<string>((resolve) => {
				const source = `fetch(${JSON.stringify(url)}).then(() => postMessage('reached'), () => postMessage('failed'))`;
				const worker = new Worker(URL.createObjectURL(new Blob([source])));
				worker.onmessage = ({ data }) => resolve(data);
			}),
		`${new URL(page.url()).origin}/api/v1/health`
	);
	expect(answer).toBe('failed');
	await expect.poll(() => failed.includes('/api/v1/health')).toBe(true);
}

test('two pages answer catalog queries with one database', async ({ page, context, server }) => {
	const { library } = server.ids;
	const snapshots: string[] = [];
	context.on('request', (request) => {
		const { pathname } = new URL(request.url());
		// A `HEAD` only sizes it up.
		if (request.method() === 'GET' && pathname.endsWith(`/libraries/${library}/catalog`))
			snapshots.push(pathname);
	});
	await page.goto('/albums');
	await synced(page, library);
	const second = await context.newPage();
	await second.goto('/albums');

	/** The titles of the tracks `page` gets from the catalog's owner. */
	const titles = async (page: Page) =>
		(await query<{ items: { title: string }[] }>(page, library, 'listTracks')).items.map(
			(t) => t.title
		);
	const fromFirst = await titles(page);
	expect(fromFirst).toHaveLength(4);
	expect(await titles(second)).toEqual(fromFirst);

	// The first page owns the database; closing it hands the files to the second, which opens them
	// without downloading anything again.
	await page.close();
	expect(await titles(second)).toEqual(fromFirst);
	expect(snapshots).toHaveLength(1);
});

test('a synced device reads its library with the server stopped', async ({ device }) => {
	const { page, context, server } = device;
	const failed = failures(context);
	await visit(page, server.ids.library);
	const albums = await query<Albums>(page, server.ids.library, 'listAlbums');
	expect(albums.items.map((a) => a.title).sort()).toEqual(['Hearth', 'Signal']);

	await server.halt();
	await workersAreCutOff(page, failed);
	await go(page, '/albums');
	for (const { title } of albums.items)
		await expect(page.getByText(title, { exact: true }).first()).toBeVisible();
	await go(page, `/albums/${server.ids.albums.Signal}`);
	await expect(page.getByRole('heading', { name: 'Signal' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Signal Part 2', exact: true })).toBeVisible();
});

test('a playlist changed on the server updates the open playlist', async ({ page, server }) => {
	const { library } = server.ids;
	const id = await createPlaylist(server, 'Road Trip', ['Signal Part 1']);
	await page.goto('/');
	await synced(page, library);
	await go(page, `/playlists/${id}`);
	await expect(page.getByRole('heading', { name: 'Road Trip' })).toBeVisible();

	// Another device renamed it; the account's feed carries that to this one.
	const path = `/libraries/${library}/playlists/${id}`;
	const { version } = await server.api(path);
	await server.api(path, { method: 'PATCH', body: { baseVersion: version, title: 'Renamed' } });
	await expect(page.getByRole('heading', { name: 'Renamed' })).toBeVisible();
});

test('a library change on the server updates the open albums page', async ({ device }) => {
	const { page, server } = device;
	await visit(page, server.ids.library);
	await go(page, '/albums');
	await expect(page.getByText('Hearth', { exact: true }).first()).toBeVisible();

	// A new album in the music folder: the scanner finds it, and the library's feed carries it here.
	const dir = join(server.home, 'music/Birch Hollow/Ember');
	mkdirSync(dir, { recursive: true });
	const tags = {
		title: 'Ember',
		album: 'Ember',
		artist: 'Birch Hollow',
		album_artist: 'Birch Hollow'
	};
	const args = ['-v', 'error', '-f', 'lavfi', '-i', 'anullsrc=r=8000:cl=mono', '-t', '5'];
	for (const [key, value] of Object.entries(tags)) args.push('-metadata', `${key}=${value}`);
	execFileSync('ffmpeg', [...args, '-c:a', 'flac', join(dir, 'Ember.flac')]);
	await expect(page.getByText('Ember', { exact: true }).first()).toBeVisible({ timeout: 20_000 });
});

test('searching offline finds a synced track', async ({ device }) => {
	const { page, context, server } = device;
	const failed = failures(context);
	await visit(page, server.ids.library);
	await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
	await expect(page).toHaveURL(/\/albums$/);

	await server.halt();
	await context.setOffline(true);
	await workersAreCutOff(page, failed);
	const searches = failed.length;
	await page.getByRole('searchbox', { name: 'Search the library' }).fill('kindl');
	await expect(page).toHaveURL(/\/search\?q=kindl/);
	await expect(page.getByText('Kindling').first()).toBeVisible();
	// Nothing was asked of the server for it, not even to fail.
	expect(failed.slice(searches).filter((path) => path.endsWith('/search'))).toEqual([]);
});

test('starting an album offline queues all its tracks', async ({ device }) => {
	const { page, context, server } = device;
	const { library, albums } = server.ids;
	const failed = failures(context);
	const user = (await server.api('/me')).id;
	await visit(page, library);
	// The album's tracks in the order its page shows them.
	const tracks = await query<{ items: { title: string }[] }>(page, library, 'listTracks', {
		albumId: albums.Signal,
		sort: 'album',
		order: 'asc'
	});
	const titles = tracks.items.map((t) => t.title);
	expect(titles).toEqual(['Signal Part 1', 'Signal Part 2', 'Signal Remix']);

	await server.halt();
	await context.setOffline(true);
	await go(page, `/albums/${albums.Signal}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: 'Queue' }).click();
	const panel = page.getByRole('complementary', { name: 'Queue' });
	const upNext = () =>
		panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts();
	await expect.poll(upNext).toEqual(titles.slice(1));
	// The start waits in the log for the server, which the browser with no network does not try.
	await expect
		.poll(() =>
			page.evaluate(async (user) => {
				const db = await new Promise<IDBDatabase>((resolve) => {
					const request = indexedDB.open(`jewelcase-outbox-${user}`);
					request.onsuccess = () => resolve(request.result);
				});
				const all = await new Promise<{ name: string }[]>((resolve) => {
					const request = db.transaction('mutations').objectStore('mutations').getAll();
					request.onsuccess = () => resolve(request.result);
				});
				db.close();
				return all.filter((mutation) => mutation.name === 'startQueue').length;
			}, user)
		)
		.toBe(1);
	expect(failed.filter((path) => path.endsWith('/me/queue'))).toEqual([]);

	// The album page gave the queue every row it shows, so only the list the device built and
	// kept, with the server away, tells the two apart.
	await expect
		.poll(() =>
			page.evaluate(
				async (user) => (await (await caches.open(`queue-lists-${user}`)).keys()).length,
				user
			)
		)
		.toBe(1);
});

test.describe('the offline shell', () => {
	test('a page not opened before loads offline on the first visit', async ({ device }) => {
		const { page, context, server } = device;
		const failed = failures(context);
		await page.goto('/');
		await synced(page, server.ids.library);
		// No reload: the worker takes over the page it was installed from.
		await page.waitForFunction(() => navigator.serviceWorker.controller !== null);

		await server.halt();
		await context.setOffline(true);
		await workersAreCutOff(page, failed);
		await go(page, `/albums/${server.ids.albums.Signal}`);
		await expect(page.getByRole('heading', { name: 'Signal' })).toBeVisible();
	});

	test('reopening offline, or with the server unreachable, shows the app', async ({ device }) => {
		const { page, context, server } = device;
		const failed = failures(context);
		const album = server.ids.albums.Signal;
		await page.goto('/');
		await synced(page, server.ids.library);
		await page.evaluate(() => navigator.serviceWorker.ready);
		await go(page, `/albums/${album}`);
		await expect(page.getByRole('heading', { name: 'Signal' })).toBeVisible();

		// Launched at the deep link with no network, as an installed app reopens where it was left.
		await server.halt();
		await context.setOffline(true);
		await workersAreCutOff(page, failed);
		await page.reload();
		await expect(page.getByRole('heading', { name: 'Signal' })).toBeVisible();

		// The browser still says it is online, as on mobile data away from the server.
		await context.setOffline(false);
		await page.reload();
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.getByText('Hearth', { exact: true }).first()).toBeVisible();
	});

	test('a cold start offline at a deep link shows the library, which the device kept as held', async ({
		device
	}) => {
		const { page, context, server } = device;
		const failed = failures(context);
		const album = server.ids.albums.Signal;
		await page.goto('/');
		await synced(page, server.ids.library);
		await page.evaluate(() => navigator.serviceWorker.ready);
		// What the next launch reads before its own owner has said anything.
		await expect
			.poll(() =>
				page.evaluate(() =>
					Object.entries(localStorage)
						.filter(([key]) => key.startsWith('jewelcase-device:'))
						.map(([, value]) => JSON.parse(value))
				)
			)
			.toEqual([[server.ids.library]]);

		await server.halt();
		await context.setOffline(true);
		await workersAreCutOff(page, failed);
		// The first tab owned the catalog; with it gone the launch has to elect itself and start its
		// worker offline, and finds the library by what the device kept.
		await page.close();
		// A tab of its own: no state yet, and the app opens straight at the album.
		const launched = await context.newPage();
		await launched.goto(`/albums/${album}`);
		await expect(launched.getByRole('heading', { name: 'Signal' })).toBeVisible();
		await expect(
			launched.getByRole('button', { name: 'Signal Part 2', exact: true })
		).toBeVisible();
	});

	test('an album seen online shows its art offline', async ({ device }) => {
		const { page, context, server } = device;
		const { library, albums } = server.ids;
		const failed = failures(context);
		// A cover beside the files, for the scanner to find.
		const cover = join(server.home, 'music/Aurora Lane/Signal/cover.jpg');
		execFileSync(
			'ffmpeg',
			['-v', 'error', '-f', 'lavfi', '-i', 'color=c=red:s=64x64'].concat(['-frames:v', '1', cover])
		);
		await expect
			.poll(async () => (await server.api(`/libraries/${library}/albums/${albums.Signal}`)).image)
			.toBeTruthy();

		await page.goto('/');
		await synced(page, library);
		await page.evaluate(() => navigator.serviceWorker.ready);
		await page.reload();
		await synced(page, library);
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.locator('img.loaded').first()).toBeVisible();
		const user = (await server.api('/me')).id;
		await expect
			.poll(() =>
				page.evaluate(
					async (user) => (await (await caches.open(`images-${user}`)).keys()).length,
					user
				)
			)
			.toBeGreaterThan(0);

		await server.halt();
		await context.setOffline(true);
		await workersAreCutOff(page, failed);
		await page.reload();
		await page.getByRole('link', { name: 'Albums', exact: true }).first().click();
		await expect(page.getByText('Signal', { exact: true }).first()).toBeVisible();
		await expect(page.locator('img.loaded').first()).toBeVisible();
		// The kept picture was shown: no request for an image was made, not even to fail.
		expect(failed.filter((path) => path.includes('/images/'))).toEqual([]);
	});
});

test('an offline device that never synced says the library is not on it yet', async ({
	page,
	context
}) => {
	// No snapshot to download, so the library never reaches the device.
	const refused: string[] = [];
	await context.route('**/api/v1/libraries/*/catalog', (route) => {
		refused.push(route.request().url());
		return route.abort();
	});
	await page.goto('/');
	await expect(page.getByRole('link', { name: 'Artists', exact: true }).first()).toBeVisible();
	await expect.poll(() => refused.length).toBeGreaterThan(0);
	// Only the browser's word that it is offline: pages still load, which a real outage would stop.
	await page.evaluate(() => Object.defineProperty(navigator, 'onLine', { get: () => false }));
	await page.getByRole('link', { name: 'Artists', exact: true }).first().click();
	await expect(page.getByRole('heading', { name: 'Not on this device yet' })).toBeVisible();
});

test('a refused realtime connection with HTTP answering still reads the server for a library not on the device', async ({
	page,
	context
}) => {
	// No snapshot to download, so the library never reaches the device.
	await context.route('**/api/v1/libraries/*/catalog', (route) => route.abort());
	// A proxy that does not upgrade WebSockets: the connection closes at once, HTTP is fine.
	await page.routeWebSocket('**/api/v1/realtime', (ws) => ws.close());
	await page.goto('/');
	await expect(page.getByRole('status').filter({ hasText: 'Offline' })).toBeVisible();
	await page.getByRole('link', { name: 'Artists', exact: true }).first().click();
	await expect(page.getByRole('heading', { level: 1, name: 'Artists' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Aurora Lane' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Not on this device yet' })).toHaveCount(0);
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
	context,
	server
}) => {
	const { library, albums } = server.ids;
	// Every request of the page and of its workers, and the realtime connections.
	const requests: string[] = [];
	context.on('request', (request) => {
		if (request.url().includes('/api/v1/')) requests.push(new URL(request.url()).pathname);
	});
	let sockets = 0;
	page.on('websocket', (ws) => {
		sockets++;
		ws.on('close', () => sockets--);
	});
	const open = () => sockets;
	await page.goto('/');
	await synced(page, library);
	await expect.poll(open).toBe(1);

	// A track has been played, so the window below starts after audio was requested.
	await go(page, `/albums/${albums.Signal}`);
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
	await expect(page.getByText('Hearth', { exact: true }).first()).toBeVisible();
	await go(page, `/albums/${albums.Signal}`);
	await page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	// A fresh start offline reads the device too.
	await page.goto('/');
	await synced(page, library);
	await go(page, '/albums');
	await expect(page.getByText('Hearth', { exact: true }).first()).toBeVisible();
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

test('admin pages say they need the server when offline', async ({ page }) => {
	await page.goto('/account');
	await page.getByRole('switch', { name: 'Work offline' }).check();
	// A fresh start offline: the account and library list are the ones kept, and the page opens.
	await page.reload();
	await expect(page.getByRole('switch', { name: 'Work offline' })).toBeChecked();
	await expect(page.getByText('Unavailable offline. This needs the server')).toBeVisible();

	// Shown, not hidden: the links stay, and each page says why it has nothing.
	const sidebar = page.getByRole('navigation', { name: 'Library' });
	for (const [name, path] of [
		['Plugins', '/admin/plugins'],
		['Libraries', '/admin/libraries']
	]) {
		await sidebar.getByRole('link', { name }).click();
		await expect(page).toHaveURL(path);
		await expect(page.getByRole('heading', { name: 'Unavailable offline' })).toBeVisible();
		await expect(page.getByText('needs the server')).toBeVisible();
		await expect(sidebar.getByRole('link', { name })).toBeVisible();
	}
});

test.describe('the device belongs to one account', () => {
	/**
	 * What the device holds for the account: who owns it, what the app starts from, the art and lists
	 * it keeps, and its log of unsent work.
	 */
	const held = (page: Page) =>
		page.evaluate(async () => ({
			owner: localStorage.getItem('jewelcase-owner'),
			started: Object.keys(localStorage).filter((key) => key.startsWith('jewelcase-user:')),
			kept: (await caches.keys()).filter((name) => /^(images|queue-lists)-/.test(name)),
			logs: (await indexedDB.databases())
				.map(({ name }) => name ?? '')
				.filter((name) => name.startsWith('jewelcase-outbox-'))
		}));

	/**
	 * Puts a kept image and queue list in `user`'s caches, as an account browsing with art and a
	 * queue has.
	 */
	const keep = (page: Page, user: string) =>
		page.evaluate(async (user) => {
			await (
				await caches.open(`images-${user}`)
			).put('/api/v1/libraries/1/images/9', new Response('x'));
			await (
				await caches.open(`queue-lists-${user}`)
			).put('/api/v1/me/queue/lists/h', new Response('{}'));
		}, user);

	/** The snapshot downloads of `library` the context makes, as they happen. */
	function downloads(context: BrowserContext, library: string) {
		const made: string[] = [];
		context.on('request', (request) => {
			// A `HEAD` only sizes it up.
			if (
				request.method() === 'GET' &&
				new URL(request.url()).pathname.endsWith(`/libraries/${library}/catalog`)
			)
				made.push(request.url());
		});
		return made;
	}

	/** The login page's form, filled and sent, for `username`. */
	async function signIn(page: Page, username: string, password: string) {
		await page.getByLabel('Username').fill(username);
		await page.getByLabel('Password').fill(password);
		await page.getByRole('button', { name: 'Log in' }).click();
	}

	test('logging out offline wipes the device and ends the session', async ({ device }) => {
		const { page, context, server } = device;
		const { library } = server.ids;
		const failed = failures(context);
		const made = downloads(context, library);
		const user = (await server.api('/me')).id;
		await visit(page, library);
		await keep(page, user);
		expect((await held(page)).started).toHaveLength(1);
		expect((await held(page)).logs).toEqual([`jewelcase-outbox-${user}`]);
		const [session] = await context.cookies();

		// The browser still says it is online, so nothing tells the app to try the server again.
		await server.halt();
		await workersAreCutOff(page, failed);
		await go(page, '/account');
		await page.getByRole('button', { name: 'Log out' }).click();
		await expect(page).toHaveURL(/\/login$/);
		await expect(page.getByRole('button', { name: 'Log in' })).toBeVisible();
		expect(await held(page)).toEqual({ owner: null, started: [], kept: [], logs: [] });
		expect(await page.evaluate(() => localStorage.getItem('jewelcase-logout-pending'))).toBe('1');

		// Online again, the app opens at the login page, without asking who is signed in, and
		// the server is told the session is over.
		await server.restart();
		const asked: string[] = [];
		context.on('request', (request) => {
			if (request.url().includes('/api/v1/'))
				asked.push(`${request.method()} ${new URL(request.url()).pathname}`);
		});
		const meAnswers: number[] = [];
		context.on('response', (response) => {
			if (new URL(response.url()).pathname === '/api/v1/me') meAnswers.push(response.status());
		});
		await page.goto('/albums');
		await expect(page.getByRole('button', { name: 'Log in' })).toBeVisible();
		await expect(page).toHaveURL(/\/login$/);
		await expect.poll(() => asked).toContain('POST /api/v1/auth/logout');
		// Who is signed in is never asked before the server has ended the session; a later ask,
		// as the login page makes once the marker is down, is answered 401.
		const me = asked.indexOf('GET /api/v1/me');
		if (me >= 0) expect(me).toBeGreaterThan(asked.indexOf('POST /api/v1/auth/logout'));
		expect(meAnswers.filter((status) => status !== 401)).toEqual([]);
		await expect
			.poll(() => page.evaluate(() => localStorage.getItem('jewelcase-logout-pending')))
			.toBeNull();
		// The browser's cookie is the session; the server no longer knows it.
		const res = await fetch(`${server.url}/api/v1/me`, {
			headers: { Cookie: `${session.name}=${session.value}` }
		});
		expect(res.status).toBe(401);

		// The library's file went with the rest: signing in again downloads it anew.
		await signIn(page, server.username, server.password);
		await synced(page, library);
		expect(made).toHaveLength(2);
	});

	/**
	 * Logs out in `page` with the server stopped and the browser still online, then starts the server
	 * again.
	 */
	async function logOutUnheard(device: Device, page: Page) {
		const { context, server } = device;
		await server.halt();
		await workersAreCutOff(page, failures(context));
		await go(page, '/account');
		await page.getByRole('button', { name: 'Log out' }).click();
		await expect(page).toHaveURL(/\/login$/);
		await server.restart();
	}

	test('the first login after a logout the server was not told of stays signed in', async ({
		device
	}) => {
		const { page, context, server } = device;
		await visit(page, server.ids.library);
		const [old] = await context.cookies();
		await logOutUnheard(device, page);

		await signIn(page, server.username, server.password);
		await expect(page.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();
		// The pending logout must not end the session the login just made.
		await page.waitForTimeout(2000);
		expect((await page.request.get('/api/v1/me')).status()).toBe(200);
		await expect(page).not.toHaveURL(/\/login/);
		// The login ended the old session first, rather than leaving it with no cookie pointing to it.
		const res = await fetch(`${server.url}/api/v1/me`, {
			headers: { Cookie: `${old.name}=${old.value}` }
		});
		expect(res.status).toBe(401);
	});

	test('a logout the server was not told of takes every open tab to the login page', async ({
		device
	}) => {
		const { page, context, server } = device;
		await visit(page, server.ids.library);
		const [session] = await context.cookies();
		const other = await context.newPage();
		await other.goto('/albums');
		await expect(other.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();

		await logOutUnheard(device, page);
		await expect(other).toHaveURL(/\/login$/);
		// Loaded again with the server back, it tells the server, which ends the session.
		await other.reload();
		await expect(other.getByRole('button', { name: 'Log in' })).toBeVisible();
		await expect
			.poll(async () => {
				const res = await fetch(`${server.url}/api/v1/me`, {
					headers: { Cookie: `${session.name}=${session.value}` }
				});
				return res.status;
			})
			.toBe(401);
	});

	test('a password change elsewhere wipes the device', async ({ device }) => {
		const { page, context, server } = device;
		const { library } = server.ids;
		const user = (await server.api('/me')).id;
		const made = downloads(context, library);
		await visit(page, library);
		await keep(page, user);

		endBrowserSessions(server.home);
		await page.reload();
		await expect(page).toHaveURL(/\/login\?next=/);
		// Wiped, then the page reloads at the login page, so nothing of the session stays in memory.
		await expect(page.getByLabel('Username')).toBeVisible();
		expect(await held(page)).toEqual({ owner: null, started: [], kept: [], logs: [] });

		// Signing in again starts from nothing: the library is downloaded anew.
		await signIn(page, server.username, server.password);
		await synced(page, library);
		expect(made).toHaveLength(2);
	});

	test('a session the server ends while the app is open wipes the device', async ({ device }) => {
		const { page, context, server } = device;
		const user = (await server.api('/me')).id;
		await visit(page, server.ids.library);
		await keep(page, user);
		const [session] = await context.cookies();

		// Another client logs this session out: the realtime connection says so.
		await fetch(`${server.url}/api/v1/auth/logout`, {
			method: 'POST',
			headers: { Cookie: `${session.name}=${session.value}`, 'X-Jewelcase-Client': 'web' }
		});
		await expect(page).toHaveURL(/\/login/);
		await expect.poll(() => held(page)).toEqual({ owner: null, started: [], kept: [], logs: [] });
	});

	test('a server that cannot be reached wipes nothing', async ({ device }) => {
		const { page, context, server } = device;
		const user = (await server.api('/me')).id;
		await visit(page, server.ids.library);
		await keep(page, user);
		const before = await held(page);

		await server.halt();
		await context.setOffline(true);
		await page.reload();
		await page.waitForTimeout(3500);
		await expect(page.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();
		expect(await held(page)).toEqual(before);
	});

	test('another account signing in starts from nothing', async ({ device }) => {
		const { page, context, server } = device;
		const { library } = server.ids;
		const made = downloads(context, library);
		const first = (await server.api('/me')).id;
		await visit(page, library);
		await keep(page, first);
		expect((await held(page)).owner).toBe(first);

		// Someone else signs in to this browser without the first account being signed out.
		addAdmin(server.home, 'guest');
		await login(page, server.url, 'guest', server.password);
		const second = (await (await page.request.get('/api/v1/me')).json()).id;
		await page.reload();
		await expect(page.getByRole('link', { name: 'Albums', exact: true }).first()).toBeVisible();
		await synced(page, library);

		// Nothing of the first account is left, and the second's library came from the server.
		await expect
			.poll(async () => {
				const { owner, started, kept, logs } = await held(page);
				const ofFirst = [...kept, ...logs].filter((name) => name.endsWith(`-${first}`));
				return { owner, started, ofFirst };
			})
			.toEqual({ owner: second, started: [`jewelcase-user:${second}`], ofFirst: [] });
		expect(made).toHaveLength(2);
	});
});
