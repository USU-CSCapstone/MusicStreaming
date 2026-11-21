import { randomUUID } from 'node:crypto';
import type { Page } from '@playwright/test';
import { createUser, expect, login, test } from './test';

/** Logs in on the login page, as a person does. */
async function logIn(page: Page, username: string, password: string) {
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Log in' }).click();
}

/** The IDs of the account's devices, by name, as the server lists them to `page`. */
async function devices(page: Page) {
	const res = await page.request.get('/api/v1/me/devices');
	expect(res.ok()).toBe(true);
	const { items } = (await res.json()) as {
		items: { id: string; name: string }[];
	};
	return Object.fromEntries(items.map((d) => [d.name, d.id]));
}

test('a password changed on the Account page signs the other browser out, and it signs in again as the same device; then a reset link from the owner sets another, once', async ({
	page,
	browser,
	server
}) => {
	const user = await createUser(server, [server.ids.library]);
	const first = await browser.newContext({ baseURL: server.url });
	const second = await browser.newContext({ baseURL: server.url });
	const third = await browser.newContext({ baseURL: server.url });
	const late = await browser.newContext({ baseURL: server.url });
	try {
		const here = await first.newPage();
		await login(here, server.url, user.username, user.password, {
			name: 'First browser',
			type: 'desktop'
		});
		const there = await second.newPage();
		await login(there, server.url, user.username, user.password, {
			name: 'Second browser',
			type: 'desktop'
		});
		await there.goto('/albums');
		await expect(there.getByText('Signal', { exact: true }).first()).toBeVisible();
		const before = await devices(there);
		expect(Object.keys(before)).toEqual(
			expect.arrayContaining(['First browser', 'Second browser'])
		);

		// The change signs the second browser out, to the login page.
		const changed = `e2e-${randomUUID()}`;
		await here.goto('/account');
		await here.getByLabel('Current password').fill(user.password);
		await here.getByLabel('New password', { exact: true }).fill(changed);
		await here.getByLabel('New password again').fill(changed);
		await here.getByRole('button', { name: 'Change password' }).click();
		await expect(here.getByText('Your password was changed')).toBeVisible();
		await expect(there).toHaveURL(/\/login(\?|$)/);

		// It signs in again with the new password, and is the device it was, not a new one.
		await logIn(there, user.username, changed);
		await expect(there).toHaveURL(/\/albums$/);
		// (The device the invite made the account on is signed out by the change, and stays so.)
		expect(await devices(there)).toEqual({
			'First browser': before['First browser'],
			'Second browser': before['Second browser']
		});

		// The owner makes a reset link; its page names the account, and the new password signs in.
		await page.goto('/admin/accounts');
		await page.getByRole('searchbox', { name: 'Search accounts' }).fill(user.username);
		await page.getByRole('button', { name: 'Search' }).click();
		const row = page.getByRole('listitem').filter({ hasText: user.username });
		await expect(row).toHaveCount(1);
		await row.getByRole('button', { name: 'Reset password' }).click();
		await page.getByLabel('Your password').fill(server.password);
		await page.getByRole('dialog').getByRole('button', { name: 'Reset password' }).click();
		const link = (await page.getByText(/\/reset#/).textContent())!;
		await expect(row.getByText('Reset pending')).toBeVisible();

		const person = await third.newPage();
		await person.goto(link);
		await expect(person.getByText(user.username, { exact: true })).toBeVisible();
		expect(person.url()).not.toContain('#');
		await expect(person.getByRole('navigation', { name: 'Library' })).toHaveCount(0);
		const reset = `e2e-${randomUUID()}`;
		await person.getByLabel('New password', { exact: true }).fill(reset);
		await person.getByLabel('New password again').fill(reset);
		await person.getByRole('button', { name: 'Change password' }).click();
		await expect(person).toHaveURL(/\/login\?reset/);
		await expect(person.getByText('Your password was changed')).toBeVisible();
		await logIn(person, user.username, reset);
		await expect(person).toHaveURL(/\/albums$/);

		// Used, the link is dead.
		const stale = await late.newPage();
		await stale.goto(link);
		await expect(stale.getByText('This link is no longer valid.')).toBeVisible();
		await expect(stale.getByLabel('New password again')).toHaveCount(0);
	} finally {
		await Promise.all([first.close(), second.close(), third.close(), late.close()]);
	}
});
