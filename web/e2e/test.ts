// `test` for the end-to-end specs: a real server per worker, and the page logged in unless
// `loggedIn` is off.

import { randomUUID } from 'node:crypto';
import { type BrowserContext, type Page, test as base } from '@playwright/test';
import { type Launched, type Server, launch, populate, startServer } from './server';

export { expect } from '@playwright/test';

const CLIENT = { 'X-Jewelcase-Client': 'web' };

const DESKTOP = { name: 'Chrome on Linux', type: 'desktop', platform: 'web' };

/**
 * Logs the page's context in to the server at `url`, as the web app does, registering `device`
 * (a desktop browser unless given).
 */
export async function login(
	page: Page,
	url: string,
	username: string,
	password: string,
	device: { name: string; type: string; platform?: string } = DESKTOP
) {
	const res = await page.request.post(`${url}/api/v1/auth/login`, {
		headers: CLIENT,
		data: { username, password, device }
	});
	if (!res.ok()) throw new Error(`Login failed: ${res.status()} ${await res.text()}`);
}

/** Creates the owner's playlist of the tracks titled `trackTitles`, in that order, and returns its ID. */
export async function createPlaylist(server: Server, title: string, trackTitles: string[]) {
	const { library, tracks } = server.ids;
	const items = trackTitles.map((name) => ({ type: 'track', id: tracks[name] }));
	const made = await server.api(`/libraries/${library}/playlists`, {
		method: 'POST',
		body: { title, items }
	});
	return made.id as string;
}

/** Whether the catalog's owner says `library` is on the device. */
export const onDevice = (page: Page, library: string) =>
	page.evaluate(
		(library) =>
			new Promise<boolean>((resolve) => {
				const channel = new BroadcastChannel('jewelcase-catalog');
				const timer = setTimeout(() => (channel.close(), resolve(false)), 1000);
				channel.onmessage = ({ data }) => {
					if (data.type !== 'state') return;
					clearTimeout(timer);
					channel.close();
					resolve(!!data.state?.libraries?.[library]?.onDevice);
				};
				channel.postMessage({ protocol: 2, type: 'who' });
			}),
		library
	);

/** Goes to `path` inside the app as a link click does, so the page is not reloaded. */
export const go = (page: Page, path: string) =>
	page.evaluate((path) => {
		const link = document.createElement('a');
		link.href = path;
		document.body.append(link);
		link.click();
		link.remove();
	}, path);

/**
 * A server of its own and a browser context logged in to it, with service workers, for specs that
 * take the server away.
 */
export interface Device {
	server: Server;
	context: BrowserContext;
	page: Page;
}

export const test = base.extend<
	{
		cleanAccount: void;
		/** Whether the page starts logged in; `test.use({ loggedIn: false })` for the login tests. */
		loggedIn: boolean;
		/** Starts a server of its own that is not set up, with or without music; stopped after the test. */
		freshServer: (options?: { music?: boolean }) => Promise<Launched>;
		/**
		 * A device on a server of its own, set up with the generated music, so a spec may stop and
		 * restart the server, or change its library, without touching the shared one. Stopped after
		 * the test.
		 */
		device: Device;
	},
	{ server: Server }
>({
	server: [
		// eslint-disable-next-line no-empty-pattern -- Playwright reads fixtures from this pattern
		async ({}, use) => {
			const server = await startServer();
			try {
				await use(server);
			} finally {
				await server.stop();
			}
		},
		{ scope: 'worker', timeout: 120_000 }
	],
	// Every test starts with no playlists but an empty Liked Songs, which every user has and the
	// server would not make again once deleted, no hidden songs, and no recent searches or
	// plugins, as the server is shared with the tests before it.
	cleanAccount: [
		async ({ server }, use) => {
			const path = `/libraries/${server.ids.library}`;
			// A page at a time, as a test may have made more than one page of them.
			for (;;) {
				const all: { id: string; kind: string }[] = (
					await server.api(`${path}/playlists?limit=1000`)
				).items;
				const items = all.filter((p) => p.kind !== 'liked');
				if (!items.length) break;
				await Promise.all(
					items.map((p) => server.api(`${path}/playlists/${p.id}`, { method: 'DELETE' }))
				);
			}
			// Liked Songs stays, emptied; the server keeps it for as long as it is not deleted.
			for (const id of Object.values(server.ids.tracks)) {
				await server.api(`${path}/tracks/${id}/like`, { method: 'DELETE' });
				await server.api(`${path}/tracks/${id}/hidden`, { method: 'DELETE' });
			}
			// The active queue cannot be deleted, so one left by a test is replaced by one on a song
			// the specs never look for in the player.
			const hadQueue = await server.api('/me/queue').then(
				() => true,
				(e: Error) => {
					if (e.message.includes(': 404 ')) return false;
					throw e;
				}
			);
			if (hadQueue) {
				await server.api('/me/queue', {
					method: 'POST',
					body: {
						queueId: randomUUID(),
						context: { type: 'songs' },
						source: {
							key: randomUUID(),
							libraryId: server.ids.library,
							tracks: [server.ids.tracks.Kindling]
						},
						start: 0
					}
				});
			}
			// Kept after the replacement above, which keeps a queue that was adjusted.
			for (;;) {
				const { items } = await server.api('/me/queues?limit=100');
				if (!items.length) break;
				for (const { queueId } of items) {
					await server.api(`/me/queues/${queueId}`, { method: 'DELETE' });
				}
			}
			await server.api(`${path}/recent-searches`, { method: 'DELETE' });
			const { items: plugins } = await server.api('/admin/plugins');
			// Uninstalling keeps a plugin's grants and settings for one installed again, so they are
			// cleared first, in every library.
			for (const { id } of plugins) {
				const body = { granted: [], libraries: [{ libraryId: server.ids.library, granted: [] }] };
				await server.api(`/admin/plugins/${id}/permissions`, { method: 'PUT', body });
				await server.api(`/admin/plugins/${id}/settings`, { method: 'PUT', body: { values: {} } });
				await server.api(`/admin/plugins/${id}`, { method: 'DELETE' });
			}
			await use();
		},
		{ auto: true }
	],
	loggedIn: [true, { option: true }],
	// eslint-disable-next-line no-empty-pattern -- Playwright reads fixtures from this pattern
	freshServer: async ({}, use) => {
		// Kept from the moment they start launching, so a test that fails mid-launch still stops them.
		const launching: Promise<Launched>[] = [];
		try {
			await use((options) => {
				const pending = launch(options?.music);
				launching.push(pending);
				return pending;
			});
		} finally {
			const settled = await Promise.allSettled(launching);
			await Promise.all(settled.map((r) => r.status === 'fulfilled' && r.value.stop()));
		}
	},
	device: async ({ browser, freshServer }, use) => {
		const server = await populate(await freshServer());
		const context = await browser.newContext({ baseURL: server.url, serviceWorkers: 'allow' });
		try {
			const page = await context.newPage();
			await login(page, server.url, server.username, server.password);
			await use({ server, context, page });
		} finally {
			await context.close();
		}
	},
	baseURL: async ({ server }, use) => use(server.url),
	// Unless `loggedIn` is off, the page's context logs in as the owner, as the web app does, and
	// logs out afterwards, unless the test failed: its page then stays as it was for the error
	// context.
	page: async ({ page, server, loggedIn }, use, testInfo) => {
		if (!loggedIn) return use(page);
		await login(page, server.url, server.username, server.password);
		await use(page);
		if (testInfo.status === testInfo.expectedStatus) {
			await page.request.post('/api/v1/auth/logout', { headers: CLIENT });
		}
	}
});
