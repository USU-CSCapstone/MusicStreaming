import type { Page } from '@playwright/test';
import type { Server } from './server';
import { expect, go, test } from './test';

// What the log set aside after it kept failing is listed on the account page, where it can be
// retried or discarded (`requirements/offline.md` §8: failures surfaced once and recoverable). The
// page's clock is moved on a week, which is what sets a mutation aside without twenty tries.

/** A week and a day in milliseconds, as the clock is moved on to set a mutation aside. */
const PAST_A_WEEK = 8 * 24 * 3600_000;

/** The like button of the song titled `title`. */
const heart = (page: Page, title: string) => page.getByRole('button', { name: `Like “${title}”` });

/** Whether the server holds the song titled `title` as liked. */
const likedOnServer = async (server: Server, title: string) =>
	(await server.api(`/libraries/${server.ids.library}/tracks/${server.ids.tracks[title]}`))
		.liked as boolean;

/**
 * Likes `title` on the album page while the server fails every like, and moves the clock on until
 * the like is set aside.
 */
async function likeUntilSetAside(page: Page, server: Server, title: string) {
	const failing = new RegExp(
		`/api/v1/libraries/\\d+/tracks/${server.ids.tracks[title]}/like(\\?|$)`
	);
	let tries = 0;
	await page.route(failing, (route) => {
		tries++;
		return route.fulfill({
			status: 500,
			contentType: 'application/problem+json',
			body: JSON.stringify({ status: 500, code: 'internal', title: 'Internal error' })
		});
	});
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await heart(page, title).click();
	await expect(heart(page, title)).toHaveAttribute('aria-pressed', 'true');
	await expect.poll(() => tries).toBe(1);
	await page.clock.fastForward(PAST_A_WEEK);
	await expect(page.getByText(`Couldn’t like “${title}”. It kept failing`)).toBeVisible();
	// Taken back on the device as it was set aside.
	await expect(heart(page, title)).toHaveAttribute('aria-pressed', 'false');
	await page.unroute(failing);
}

/** The account page's entry for the like of `title` set aside. */
const entry = (page: Page, title: string) =>
	page
		.getByRole('region', { name: 'Changes not sent' })
		.getByRole('listitem')
		.filter({ hasText: `Like “${title}”` });

test('a like set aside is listed with why, and Retry sends it', async ({ page, server }) => {
	await page.clock.install();
	await likeUntilSetAside(page, server, 'Signal Part 1');

	await go(page, '/account');
	await expect(entry(page, 'Signal Part 1')).toContainText('It couldn’t be sent for a week.');
	await entry(page, 'Signal Part 1').getByRole('button', { name: 'Retry' }).click();
	await expect(page.getByRole('region', { name: 'Changes not sent' })).toBeHidden();
	await expect.poll(() => likedOnServer(server, 'Signal Part 1')).toBe(true);

	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await expect(heart(page, 'Signal Part 1')).toHaveAttribute('aria-pressed', 'true');
});

test('a like set aside and discarded is never sent, and stays taken back', async ({
	page,
	server
}) => {
	await page.clock.install();
	await likeUntilSetAside(page, server, 'Signal Part 2');
	const requests: string[] = [];
	page.on('request', (request) => {
		if (request.url().includes('/like')) requests.push(request.url());
	});

	await go(page, '/account');
	await entry(page, 'Signal Part 2').getByRole('button', { name: 'Discard' }).click();
	await expect(page.getByRole('region', { name: 'Changes not sent' })).toBeHidden();
	// Still gone after a reload, and nothing went to the server.
	await page.reload();
	await expect(page.getByRole('heading', { name: 'Account' })).toBeVisible();
	await expect(page.getByRole('region', { name: 'Changes not sent' })).toBeHidden();
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await expect(heart(page, 'Signal Part 2')).toHaveAttribute('aria-pressed', 'false');
	expect(requests).toEqual([]);
	expect(await likedOnServer(server, 'Signal Part 2')).toBe(false);
});
