// `test` for specs that run against the real server: one server per worker, the page logged in.

import { test as base } from '@playwright/test';
import { type Server, startServer } from './server';

export { expect } from '@playwright/test';

const CLIENT = { 'X-Jewelcase-Client': 'web' };

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

export const test = base.extend<{ cleanPlaylists: void }, { server: Server }>({
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
	// Every test starts with no playlists, as the server may be shared with the tests before it.
	cleanPlaylists: [
		async ({ server }, use) => {
			const path = `/libraries/${server.ids.library}/playlists`;
			for (const { id } of (await server.api(`${path}?limit=200`)).items) {
				await server.api(`${path}/${id}`, { method: 'DELETE' });
			}
			await use();
		},
		{ auto: true }
	],
	baseURL: async ({ server }, use) => use(server.url),
	// The page's context logs in as the owner, as the web app does, and logs out afterwards.
	page: async ({ page, server }, use) => {
		const { username, password } = server;
		const login = await page.request.post('/api/v1/auth/login', {
			headers: CLIENT,
			data: {
				username,
				password,
				device: { name: 'Chrome on Linux', type: 'desktop', platform: 'web' }
			}
		});
		if (!login.ok()) throw new Error(`Login failed: ${login.status()} ${await login.text()}`);
		await use(page);
		await page.request.post('/api/v1/auth/logout', { headers: CLIENT });
	}
});
