import { expect, test } from '@playwright/test';
import { serveLibrary } from './fixtures';

test('an admin scans a library and watches it through to the end', async ({ page }) => {
	const { scanClock } = await serveLibrary(page);
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Libraries' }).click();
	await expect(page).toHaveURL(/\/admin\/libraries$/);
	const card = page.getByRole('listitem').filter({ hasText: 'Test Library' });
	await expect(card.getByText('3 songs · 1 album · 1 artist')).toBeVisible();
	await expect(card.getByText('Not scanned yet')).toBeVisible();

	// Queued at once, before the server answers (`requirements/general.md` §3.4).
	let answer!: () => void;
	const held = new Promise<void>((resolve) => (answer = resolve));
	await page.route('**/api/v1/admin/libraries/*/scans', async (route) => {
		if (route.request().method() === 'POST') await held;
		await route.fallback();
	});
	await card.getByRole('button', { name: 'Scan now: Test Library' }).click();
	await expect(card.getByRole('button', { name: 'Queued: Test Library' })).toBeDisabled();
	await expect(card.getByText('About to start: Manual scan')).toBeVisible();
	answer();

	await expect(card.getByText('Manual scan · Looking for files')).toBeVisible();
	await expect(card.getByText('In Aurora Lane/Signal')).toBeVisible();
	scanClock.now = 2000;
	await expect(card.getByText('Manual scan · Read 2 of 3 files found so far')).toBeVisible();

	// A press while it runs queues one to follow it, and another would add nothing.
	await card.getByRole('button', { name: 'Scan now: Test Library' }).click();
	await expect(card.getByText('Up next: Manual scan')).toBeVisible();
	await expect(card.getByRole('button', { name: 'Queued: Test Library' })).toBeDisabled();

	// The first finishes, and the one queued behind it starts.
	scanClock.now = 3000;
	await expect(card.getByText('Manual scan · Looking for files')).toBeVisible();
	await expect(card.getByText(/^Up next/)).toBeHidden();
	await expect(card.getByRole('button', { name: 'Scan now: Test Library' })).toBeEnabled();

	scanClock.now = 6000;
	await expect(card.getByText(/^Last scanned .*: nothing changed$/)).toBeVisible();
	await expect(card.getByText('Manual scan ·')).toBeHidden();

	// With nothing under way, the page stops asking, once it has reloaded the library's counts.
	let asked = 0;
	page.on('request', (r) => {
		if (r.url().endsWith('/scans')) asked++;
	});
	await page.waitForTimeout(1200);
	asked = 0;
	await page.waitForTimeout(2500);
	expect(asked).toBe(0);
});

test('a press the server refuses is undone, and says why', async ({ page }) => {
	await serveLibrary(page);
	await page.route('**/api/v1/admin/libraries/*/scans', (route) =>
		route.request().method() === 'POST'
			? route.fulfill({
					status: 500,
					json: { status: 500, code: 'internal', detail: 'The scan could not be recorded.' }
				})
			: route.fallback()
	);
	await page.goto('/admin/libraries');
	const card = page.getByRole('listitem').filter({ hasText: 'Test Library' });
	await card.getByRole('button', { name: 'Scan now: Test Library' }).click();
	await expect(card.getByRole('alert')).toHaveText('The scan could not be recorded.');
	await expect(card.getByRole('button', { name: 'Scan now: Test Library' })).toBeEnabled();
	await expect(card.getByText('Not scanned yet')).toBeVisible();
});

test('on a phone, an admin reaches Libraries from their account', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await serveLibrary(page);
	await page.goto('/account');
	const admin = page.getByRole('region', { name: 'Administration' });
	await admin.getByRole('link', { name: 'Libraries' }).click();
	await expect(page).toHaveURL(/\/admin\/libraries$/);
	await expect(page.getByRole('button', { name: 'Scan now: Test Library' })).toBeVisible();
});

test('a user is offered neither Libraries nor the rest of administration', async ({ page }) => {
	await serveLibrary(page, { role: 'user' });
	await page.goto('/albums');
	await expect(page.getByRole('heading', { name: 'Albums' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Libraries' })).toHaveCount(0);
	await page.goto('/account');
	await expect(page.getByRole('heading', { name: 'Devices' })).toBeVisible();
	await expect(page.getByRole('region', { name: 'Administration' })).toHaveCount(0);
});
