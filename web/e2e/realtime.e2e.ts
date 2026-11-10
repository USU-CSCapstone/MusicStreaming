import type { Page } from '@playwright/test';
import type { PlayerState, RealtimeClientMessage } from '../src/lib/api/types';
import { expect, login, test as base } from './test';

// Realtime against a real server: another device is a second browser context on the same account,
// a phone that plays Part 2, and the commands between the two go through the server.

const PHONE = { name: 'Pixel Phone', type: 'phone', platform: 'web' };

const test = base.extend<{
	/**
	 * Logs a phone in to the account, which plays Signal Part 2 once the album is open. With
	 * `dropCommands` its connection throws away the commands the server passes on, as a phone that
	 * does not answer. Logged out and closed after the test.
	 */
	phone: (options?: { dropCommands?: boolean }) => Promise<Page>;
}>({
	phone: async ({ browser, server }, use) => {
		const contexts: Page[] = [];
		await use(async ({ dropCommands = false } = {}) => {
			const context = await browser.newContext({ baseURL: server.url });
			const page = await context.newPage();
			contexts.push(page);
			await login(page, server.url, server.username, server.password, PHONE);
			if (dropCommands) {
				await page.routeWebSocket('**/api/v1/realtime', (ws) => {
					const upstream = ws.connectToServer();
					ws.onMessage((data) => upstream.send(data));
					upstream.onMessage((data) => {
						if (JSON.parse(String(data)).type !== 'execute') ws.send(data);
					});
				});
			}
			await page.goto(`/albums/${server.ids.albums.Signal}`);
			await page.getByRole('button', { name: 'Signal Part 2', exact: true }).dblclick();
			await expect(
				page.getByRole('region', { name: 'Player' }).getByText('Signal Part 2')
			).toBeVisible();
			await expect.poll(async () => (await player(page)).status).toBe('playing');
			return page;
		});
		for (const page of contexts) {
			await page.request.post('/api/v1/auth/logout', { headers: { 'X-Jewelcase-Client': 'web' } });
			await page.context().close();
		}
	}
});

/** The account's playback state, as the server has it, read as the page's device. */
async function player(page: Page): Promise<PlayerState> {
	return (await page.request.get('/api/v1/me/player')).json();
}

/** The ID the server gave the page's own device. */
async function deviceId(page: Page): Promise<string> {
	const { items } = await (await page.request.get('/api/v1/me/devices')).json();
	return items.find((d: { current?: boolean }) => d.current).id;
}

/** The messages the page sends on its realtime connection from now on. */
function sentMessages(page: Page) {
	const sent: RealtimeClientMessage[] = [];
	page.on('websocket', (socket) =>
		socket.on('framesent', ({ payload }) => sent.push(JSON.parse(String(payload))))
	);
	return sent;
}

/** The page's player bar. */
const bar = (page: Page) => page.getByRole('region', { name: 'Player' });

test('shows what another device plays, and lists this device first', async ({ page, phone }) => {
	await phone();
	await page.goto('/albums');

	await expect(bar(page).getByText('Signal Part 2')).toBeVisible();
	await expect(bar(page).getByText(`On ${PHONE.name}`)).toBeVisible();

	await bar(page)
		.getByRole('button', { name: `Devices, playing on ${PHONE.name}` })
		.click();
	const devices = page.getByRole('list', { name: 'Devices' }).getByRole('listitem');
	// Devices other logins left on the server are listed too, so the phone is found by name.
	await expect(devices.first()).toContainText('Chrome on Linux');
	await expect(devices.first()).toContainText('This device · Connected');
	await expect(devices.filter({ hasText: PHONE.name })).toContainText('Playing · Signal Part 2');
});

test('pausing in the bar pauses the other device', async ({ page, phone }) => {
	const other = await phone();
	const sent = sentMessages(page);
	await page.goto('/albums');

	await expect(bar(page).getByText(`On ${PHONE.name}`)).toBeVisible();
	await bar(page).getByRole('button', { name: 'Pause' }).click();

	await expect(bar(page).getByRole('button', { name: 'Play', exact: true })).toBeVisible();
	const commands = () => sent.flatMap((m) => (m.type === 'playerCommand' ? [m.command] : []));
	await expect.poll(commands).toEqual([expect.objectContaining({ type: 'pause' })]);
	// The phone really paused: it told the server, and shows it.
	await expect.poll(async () => (await player(page)).status).toBe('paused');
	await expect(bar(other).getByRole('button', { name: 'Play', exact: true })).toBeVisible();
	await expect(bar(page).getByText(`On ${PHONE.name}`)).toBeVisible();
});

test('says why the other device did not do what was asked', async ({ page, phone }) => {
	await phone({ dropCommands: true });
	await page.goto('/albums');

	await bar(page).getByRole('button', { name: 'Next' }).click();
	await expect(bar(page).getByRole('alert')).toHaveText('The device did not answer.', {
		timeout: 15_000
	});
	await expect(bar(page).getByText('Signal Part 2')).toBeVisible();
});

test('playing here takes playback over from the other device', async ({ page, phone }) => {
	const other = await phone();
	const sent = sentMessages(page);
	const before = await player(page);
	await page.goto('/albums');

	await bar(page)
		.getByRole('button', { name: `Devices, playing on ${PHONE.name}` })
		.click();
	await page.getByRole('button', { name: 'Play here' }).click();

	await expect(bar(page).getByText(`On ${PHONE.name}`)).toBeHidden();
	await expect(bar(page).getByText('Signal Part 2')).toBeVisible();
	const takeovers = () => sent.flatMap((m) => (m.type === 'progress' ? [m.report] : []));
	await expect
		.poll(takeovers)
		.toContainEqual(expect.objectContaining({ start: true, itemId: before.itemId }));
	const here = await deviceId(page);
	await expect.poll(async () => (await player(page)).activeDeviceId).toBe(here);
	await expect(bar(page).getByRole('button', { name: 'Devices', exact: true })).toBeVisible();
	// The button gone, focus stays in the list, which says where playback went.
	await expect(page.getByRole('list', { name: 'Devices' })).toBeFocused();
	await expect(page.getByRole('status').filter({ hasText: 'Playback is' })).toHaveText(
		'Playback is on this device'
	);
	// The phone stopped, and offers playback back from this device.
	await expect(bar(other).getByText('On Chrome on Linux')).toBeVisible();
});

test('a song played here from a view takes playback over from the other device', async ({
	page,
	phone,
	server
}) => {
	await phone();
	const before = await player(page);
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await bar(page).getByText('Signal Part 2').waitFor();

	await page.locator('main').getByText('Signal Remix').dblclick();
	const here = await deviceId(page);
	// The server has the queue before it hears of this device at an item of it.
	await expect.poll(async () => (await player(page)).activeDeviceId).toBe(here);
	expect((await player(page)).itemId).not.toBe(before.itemId);
});
