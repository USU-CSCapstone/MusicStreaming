import type { Page } from '@playwright/test';
import { expect, test } from './test';

// Scanning a library from the Libraries page (`requirements/scanning.md` §5). A real scan of the
// four generated tracks ends within a poll, so a running scan, and one queued behind it, are not
// seen here: their wording is left to the unit tests of `$lib/api/scans`, and how scans queue to
// the server's. That a user is offered no Libraries waits for accounts other than the owner.

/** The card of the server's one library, which is named "Music". */
const card = (page: Page) =>
	page
		.getByRole('listitem')
		.filter({ has: page.getByRole('heading', { name: 'Music', exact: true }) });

test('an admin scans a library and watches it through to the end', async ({ page, server }) => {
	// The shared server may still be scanning changed files, as after a plugin saves into the music.
	await expect
		.poll(async () => (await server.api(`/libraries/${server.ids.library}`)).scanning)
		.toBe(false);
	const scans = `/admin/libraries/${server.ids.library}/scans`;
	const before = new Set((await server.api(scans)).items.map((s: { id: string }) => s.id));
	await page.goto('/albums');
	await page.getByRole('link', { name: 'Libraries' }).click();
	await expect(page).toHaveURL(/\/admin\/libraries$/);
	await expect(card(page).getByText('4 songs · 2 albums · 2 artists')).toBeVisible();
	await expect(card(page).getByText(/^Last scanned /)).toBeVisible();

	// Queued at once, before the server answers (`requirements/general.md` §3.4).
	let answer!: () => void;
	const held = new Promise<void>((resolve) => (answer = resolve));
	await page.route('**/api/v1/admin/libraries/*/scans', async (route) => {
		if (route.request().method() === 'POST') await held;
		await route.fallback();
	});
	await card(page).getByRole('button', { name: 'Scan now: Music' }).click();
	await expect(card(page).getByRole('button', { name: 'Queued: Music' })).toBeDisabled();
	await expect(card(page).getByText('About to start: Manual scan')).toBeVisible();
	answer();

	// The files are as they were, so the scan finds nothing to change. The page shows when this
	// scan finished, as an earlier scan may have changed nothing too.
	await expect
		.poll(async () => {
			const [latest] = (await server.api(scans)).items;
			return !before.has(latest.id) && latest.state === 'completed';
		}, 'a scan of its own, finished')
		.toBe(true);
	const [latest] = (await server.api(scans)).items;
	expect(latest).toMatchObject({ trigger: 'manual', wholeLibrary: true });
	const at = await page.evaluate((d) => new Date(d).toLocaleString(), latest.finishedAt);
	await expect(card(page).getByText(`Last scanned ${at}: nothing changed`)).toBeVisible();
	await expect(card(page).getByRole('button', { name: 'Scan now: Music' })).toBeEnabled();

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
	await page.route('**/api/v1/admin/libraries/*/scans', (route) =>
		route.request().method() === 'POST'
			? route.fulfill({
					status: 500,
					json: { status: 500, code: 'internal', detail: 'The scan could not be recorded.' }
				})
			: route.fallback()
	);
	await page.goto('/admin/libraries');
	await card(page).getByRole('button', { name: 'Scan now: Music' }).click();
	await expect(card(page).getByRole('alert')).toHaveText('The scan could not be recorded.');
	await expect(card(page).getByRole('button', { name: 'Scan now: Music' })).toBeEnabled();
	await expect(card(page).getByText(/^Last scanned /)).toBeVisible();
});

test('on a phone, an admin reaches Libraries from their account', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto('/account');
	const admin = page.getByRole('region', { name: 'Administration' });
	await admin.getByRole('link', { name: 'Libraries' }).click();
	await expect(page).toHaveURL(/\/admin\/libraries$/);
	await expect(page.getByRole('button', { name: 'Scan now: Music' })).toBeVisible();
});
