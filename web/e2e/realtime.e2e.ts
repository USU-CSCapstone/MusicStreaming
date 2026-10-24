import { expect, test } from '@playwright/test';
import { phone, serveLibrary, serveRealtime } from './fixtures';

test('shows what another device plays, and lists this device first', async ({ page }) => {
	await serveLibrary(page);
	await serveRealtime(page);
	await page.goto('/albums');

	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await expect(player.getByText(`On ${phone.name}`)).toBeVisible();

	await player.getByRole('button', { name: `Devices, playing on ${phone.name}` }).click();
	const devices = page.getByRole('list', { name: 'Devices' }).getByRole('listitem');
	await expect(devices).toHaveCount(2);
	await expect(devices.first()).toContainText('Chrome on Linux');
	await expect(devices.first()).toContainText('This device · Connected');
	await expect(devices.nth(1)).toContainText('Playing · Signal Part 2');
});

test('pausing in the bar pauses the other device', async ({ page }) => {
	await serveLibrary(page);
	const { sent } = await serveRealtime(page);
	await page.goto('/albums');

	const player = page.getByRole('region', { name: 'Player' });
	await expect(player.getByText(`On ${phone.name}`)).toBeVisible();
	await player.getByRole('button', { name: 'Pause' }).click();

	await expect(player.getByRole('button', { name: 'Play', exact: true })).toBeVisible();
	const commands = () => sent.flatMap((m) => (m.type === 'playerCommand' ? [m.command] : []));
	await expect.poll(commands).toEqual([expect.objectContaining({ type: 'pause' })]);
	await expect(player.getByText(`On ${phone.name}`)).toBeVisible();
});

test('says why the other device did not do what was asked', async ({ page }) => {
	await serveLibrary(page);
	await serveRealtime(page);
	await page.goto('/albums');

	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: 'Next' }).click();
	await expect(player.getByRole('alert')).toHaveText('The device did not answer.');
	await expect(player.getByText('Signal Part 2')).toBeVisible();
});

test('playing here takes playback over from the other device', async ({ page }) => {
	await serveLibrary(page);
	const { sent } = await serveRealtime(page);
	await page.goto('/albums');

	const player = page.getByRole('region', { name: 'Player' });
	await player.getByRole('button', { name: `Devices, playing on ${phone.name}` }).click();
	await page.getByRole('button', { name: 'Play here' }).click();

	await expect(player.getByText(`On ${phone.name}`)).toBeHidden();
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	const takeovers = () => sent.flatMap((m) => (m.type === 'progress' ? [m.report] : []));
	await expect
		.poll(takeovers)
		.toContainEqual(expect.objectContaining({ start: true, trackId: '42' }));
	await expect(player.getByRole('button', { name: 'Devices', exact: true })).toBeVisible();
	// The button gone, focus stays in the list, which says where playback went.
	await expect(page.getByRole('list', { name: 'Devices' })).toBeFocused();
	await expect(page.getByRole('status').filter({ hasText: 'Playback is' })).toHaveText(
		'Playback is on this device'
	);
});
