import type { Page } from '@playwright/test';
import { playsHeld } from './server';
import { expect, onDevice, test } from './test';

// What a device did while the server was out of reach is kept by the app and sent once it opens
// again with the server back (`requirements/offline.md` §8). The server is stopped, which is what
// an outage looks like to the page; the browser, and so its database, survive a restart.

/** The arguments of the mutations named `name` the page's log holds, as stored. */
const kept = (page: Page, name: 'play' | 'mark' | 'editQueue') =>
	page.evaluate(async (name) => {
		const names = (await indexedDB.databases()).map((db) => db.name!);
		const all: { name?: string; args?: unknown }[] = [];
		const store = 'mutations';
		for (const db of names.filter((n) => n.startsWith('jewelcase-outbox-'))) {
			const open = await new Promise<IDBDatabase>((resolve) => {
				const request = indexedDB.open(db);
				request.onsuccess = () => resolve(request.result);
			});
			all.push(
				...(await new Promise<{ name?: string; args?: unknown }[]>((resolve) => {
					const request = open.transaction(store).objectStore(store).getAll();
					request.onsuccess = () => resolve(request.result);
				}))
			);
			open.close();
		}
		return all.filter((m) => m.name === name).map((m) => m.args) as {
			liked?: boolean;
			listenTimeMs?: number;
		}[];
	}, name);

test('a like and a play made offline reach the server after the app reopens online', async ({
	device
}) => {
	const { page, context, server } = device;
	const { library, albums, tracks } = server.ids;
	const failed: string[] = [];
	context.on('requestfailed', (request) => {
		if (request.url().includes('/api/v1/')) failed.push(new URL(request.url()).pathname);
	});
	/** The listen time in milliseconds the server holds for the second track of the album, if any. */
	const played = () =>
		playsHeld(server.home).find((play) => play.track === tracks['Signal Part 2'])?.listenTimeMs;

	await page.goto(`/albums/${albums.Signal}`);
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).dblclick();
	await expect(
		page.getByRole('region', { name: 'Player' }).getByText('Signal Part 2')
	).toBeVisible();
	// The play began with the server there; what is heard from here on is told to nobody.
	await expect.poll(played).toBe(0);

	await server.halt();
	const heart = page.getByRole('button', { name: 'Like “Signal Part 1”' });
	await heart.click();
	await expect(heart).toHaveAttribute('aria-pressed', 'true');
	await page.waitForTimeout(2500);
	await page.getByRole('region', { name: 'Player' }).getByRole('button', { name: 'Pause' }).click();
	// Both were tried and could not go, and both are in the database, which outlives the page.
	await expect
		.poll(() => failed.some((path) => path.endsWith(`/tracks/${tracks['Signal Part 1']}/like`)))
		.toBe(true);
	await expect.poll(() => failed.some((path) => path.endsWith('/me/plays'))).toBe(true);
	await expect
		.poll(async () => (await kept(page, 'play')).map((p) => p.listenTimeMs! > 2000))
		.toEqual([true]);
	expect((await kept(page, 'mark')).map((m) => m.liked)).toEqual([true]);
	expect(played()).toBe(0);

	// The app is closed, and opened again with the server back.
	await page.close();
	await server.restart();
	const reopened = await context.newPage();
	await reopened.goto('/');

	await expect.poll(played).toBeGreaterThan(2000);
	await expect
		.poll(
			async () =>
				(await server.api(`/libraries/${library}/tracks/${tracks['Signal Part 1']}`)).liked
		)
		.toBe(true);
	await expect.poll(async () => (await kept(reopened, 'mark')).length).toBe(0);

	// Sent, the like is the device's own copy's to show: it comes back through the account feed.
	// The library reaches the device first: its download may have been cut short by the outage.
	// Loaded again working offline, nothing holds it in memory and the device alone answers,
	// waiting for an apply under way; loaded online between tries, the device catches up.
	const liked = reopened.getByRole('button', { name: 'Like “Signal Part 1”' });
	const workOffline = (on: boolean) =>
		reopened.evaluate((on) => {
			if (on) localStorage.setItem('jewelcase-work-offline', '1');
			else localStorage.removeItem('jewelcase-work-offline');
		}, on);
	await reopened.goto(`/albums/${albums.Signal}`);
	await expect.poll(() => onDevice(reopened, library), { timeout: 20_000 }).toBe(true);
	await expect
		.poll(
			async () => {
				await workOffline(true);
				await reopened.reload();
				const pressed = await liked.getAttribute('aria-pressed', { timeout: 5_000 });
				await workOffline(false);
				await reopened.reload();
				return pressed;
			},
			{ timeout: 20_000 }
		)
		.toBe('true');
});

test('a queue edit made offline is on the server after reopening online', async ({ device }) => {
	const { page, context, server } = device;
	const { Signal } = server.ids.albums;
	const remix = server.ids.tracks['Signal Remix'];
	const player = page.getByRole('region', { name: 'Player' });
	const panel = page.getByRole('complementary', { name: 'Queue' });
	/** The heading of the Manual Queue holding one track, in `p`'s queue panel. */
	const manual = (p: Page) =>
		p
			.getByRole('complementary', { name: 'Queue' })
			.getByRole('heading', { name: /Queued by you\s*1/ });
	/** The tracks of each list the server's queue holds, none before the start reaches it. */
	const lists = async () => {
		const queue = await server.api('/me/queue').catch(() => null);
		if (!queue) return [];
		return Promise.all(
			queue.state.sources.map(
				async ({ list }: { list: string }) => (await server.api(`/me/queue/lists/${list}`)).tracks
			)
		);
	};

	await page.goto(`/albums/${Signal}`);
	await page.getByRole('main').getByRole('button', { name: 'Play', exact: true }).click();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('tab').first()).toHaveText(/Up Next [1-9]/);
	await expect.poll(async () => (await lists()).length).toBe(1);
	await panel.getByRole('button', { name: 'Close queue' }).click();

	await server.halt();
	await page.getByRole('main').getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(manual(page)).toBeVisible();
	// It is in the database, which outlives the page, though the server has not heard of it.
	await expect.poll(async () => (await kept(page, 'editQueue')).length).toBe(1);

	// The app is closed, and opened again with the server back.
	await page.close();
	await server.restart();
	await (await context.newPage()).goto('/');

	await expect.poll(async () => (await lists()).length).toBe(2);
	expect((await lists())[1]).toEqual([remix]);
});
