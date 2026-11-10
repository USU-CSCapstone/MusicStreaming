// `test` for specs that run against the real server: one server per worker, and the page logged
// in unless `loggedIn` is off.

import { type Page, test as base } from '@playwright/test';
import { type Launched, type Server, launch, startServer } from './server';

export { expect } from '@playwright/test';

const CLIENT = { 'X-Jewelcase-Client': 'web' };

/** Logs the page's context in to the server at `url`, as the web app does. */
export async function login(page: Page, url: string, username: string, password: string) {
	const res = await page.request.post(`${url}/api/v1/auth/login`, {
		headers: CLIENT,
		data: {
			username,
			password,
			device: { name: 'Chrome on Linux', type: 'desktop', platform: 'web' }
		}
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

export const test = base.extend<
	{
		cleanAccount: void;
		/** Whether the page starts logged in; `test.use({ loggedIn: false })` for the login tests. */
		loggedIn: boolean;
		/** Starts a server of its own that is not set up, with or without music; stopped after the test. */
		freshServer: (options?: { music?: boolean }) => Promise<Launched>;
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
	// Every test starts with no playlists or recent searches, as the server is shared with the
	// tests before it.
	cleanAccount: [
		async ({ server }, use) => {
			const path = `/libraries/${server.ids.library}`;
			// A page at a time, as a test may have made more than one page of them.
			for (;;) {
				const { items } = await server.api(`${path}/playlists?limit=1000`);
				if (!items.length) break;
				await Promise.all(
					items.map((p: { id: string }) =>
						server.api(`${path}/playlists/${p.id}`, { method: 'DELETE' })
					)
				);
			}
			await server.api(`${path}/recent-searches`, { method: 'DELETE' });
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
	baseURL: async ({ server }, use) => use(server.url),
	// Unless `loggedIn` is off, the page's context logs in as the owner, as the web app does, and
	// logs out afterwards.
	page: async ({ page, server, loggedIn }, use) => {
		if (!loggedIn) return use(page);
		await login(page, server.url, server.username, server.password);
		await use(page);
		await page.request.post('/api/v1/auth/logout', { headers: CLIENT });
	}
});
