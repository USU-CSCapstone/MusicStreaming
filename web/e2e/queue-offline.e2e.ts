import type { Page } from '@playwright/test';
import type { StartQueue } from '../src/lib/api/types';
import { expect, test } from './test';

// Queues started and changed with the server stopped behave as they do online
// (`requirements/offline.md` §6, `requirements/queue.md` §6): nothing is refused, and once the
// server is back it has every queue a start replaced after it was adjusted, with its changes, kept,
// and the last one active. The server is stopped, which is what an outage looks like to the page.

/** Whether the catalog's owner says `library` is on the device. */
const onDevice = (page: Page, library: string) =>
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
const go = (page: Page, path: string) =>
	page.evaluate((path) => {
		const link = document.createElement('a');
		link.href = path;
		document.body.append(link);
		link.click();
		link.remove();
	}, path);

test('queues started and adjusted with the server stopped are kept on it, with their changes, once it is back', async ({
	device
}) => {
	const { page, server } = device;
	const { library, albums, tracks } = server.ids;
	const hearth = (await server.api(`/libraries/${library}/albums?limit=100`)).items.find(
		(album: { title: string }) => album.title === 'Hearth'
	).id as string;
	const player = page.getByRole('region', { name: 'Player' });
	const panel = page.getByRole('complementary', { name: 'Queue' });
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	const titles = () => upNext.locator('[data-item] .title').allInnerTexts();
	const play = () =>
		page.locator('main').getByRole('button', { name: 'Play', exact: true }).click();
	/** The queues of the starts the page tried to send, in the order it tried them. */
	const starts: string[] = [];
	page.on('request', (request) => {
		const path = new URL(request.url()).pathname;
		if (request.method() === 'POST' && path === '/api/v1/me/queue')
			starts.push((request.postDataJSON() as StartQueue).queueId);
	});
	await page.goto(`/albums/${albums.Signal}`);
	// What the toasts say, kept, as they come and go.
	await page.evaluate(() => {
		const seen: string[] = [];
		(window as unknown as { toasts: string[] }).toasts = seen;
		new MutationObserver(() => {
			for (const el of document.querySelectorAll('[role="status"]'))
				if (el.textContent) seen.push(el.textContent);
		}).observe(document.body, { subtree: true, childList: true, characterData: true });
	});

	await expect(page.locator('main').getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => onDevice(page, library), { timeout: 30_000 }).toBe(true);
	await go(page, `/albums/${hearth}`);
	await expect(page.locator('main').getByText('Kindling')).toBeVisible();
	await go(page, `/albums/${albums.Signal}`);
	await expect(page.locator('main').getByText('Signal Remix')).toBeVisible();

	await server.halt();
	await expect(player.getByText('Offline')).toBeVisible({ timeout: 15_000 });

	// The first queue: Remix moved up, then a skip to it.
	await play();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	await upNext.getByRole('button', { name: 'Signal Remix', exact: true }).focus();
	await page.keyboard.press('Alt+ArrowUp');
	await expect.poll(titles).toEqual(['Signal Remix', 'Signal Part 2']);
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByText('Signal Remix')).toBeVisible();
	await expect.poll(titles).toEqual(['Signal Part 2']);
	await panel.getByRole('button', { name: 'Close queue' }).click();

	// The second: Hearth, its song queued again, which adjusts it.
	await go(page, `/albums/${hearth}`);
	await expect(page.locator('main').getByText('Kindling')).toBeVisible();
	await play();
	await expect(player.getByText('Kindling')).toBeVisible();
	await page.locator('main').getByRole('button', { name: 'Actions for Kindling' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await panel.getByRole('button', { name: 'Close queue' }).click();

	// The third, left as it started.
	await go(page, `/albums/${albums.Signal}`);
	await expect(page.locator('main').getByText('Signal Remix')).toBeVisible();
	await play();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	// Nothing was refused, here or in a toast.
	await expect(panel.getByRole('alert')).toHaveCount(0);
	// The first start waits to be sent, and the rest of the queue's changes behind it, in order.
	expect(new Set(starts).size).toBe(1);

	await server.restart();
	const ids = () => [...new Set(starts)];
	await expect.poll(() => ids().length, { timeout: 60_000 }).toBe(3);
	await expect
		.poll(async () => (await server.api('/me/queue').catch(() => null))?.queueId)
		.toBe(ids()[2]);
	const kept = async () =>
		(
			(await server.api('/me/queues')).items as {
				queueId: string;
				current: { trackId: string } | null;
				upNext: number;
				history: number;
			}[]
		).map(({ queueId, current, upNext, history }) => ({
			queueId,
			current: current?.trackId,
			upNext,
			history
		}));
	// Both were parked within the same moment, so their order in the list is not asserted.
	const byQueue = async () =>
		(await kept()).sort((a, b) => ids().indexOf(a.queueId) - ids().indexOf(b.queueId));
	await expect.poll(byQueue, { timeout: 10_000 }).toEqual([
		{ queueId: ids()[0], current: tracks['Signal Remix'], upNext: 1, history: 1 },
		{ queueId: ids()[1], current: tracks.Kindling, upNext: 1, history: 0 }
	]);
	await expect(panel.getByRole('alert')).toHaveCount(0);
	const toasts = await page.evaluate(() => (window as unknown as { toasts: string[] }).toasts);
	expect(toasts.filter((text) => /Couldn’t|did not apply|refused/.test(text))).toEqual([]);
});
