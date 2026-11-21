import type { Page } from '@playwright/test';
import { createUser, expect, login, test } from './test';

/** The row of the account `username` on the Accounts page the page is on, after a search. */
async function accountRow(page: Page, username: string) {
	await page.goto('/admin/accounts');
	await page.getByRole('searchbox', { name: 'Search accounts' }).fill(username);
	await page.getByRole('button', { name: 'Search' }).click();
	const row = page.getByRole('listitem').filter({ hasText: username });
	await expect(row).toHaveCount(1);
	return row;
}

/** Presses `action` on the row and confirms in the dialog with the owner's password. */
async function confirm(page: Page, action: string, password: string) {
	const dialog = page.getByRole('dialog');
	await dialog.getByLabel('Your password').fill(password);
	await dialog.getByRole('button', { name: action }).click();
	await expect(dialog).toHaveCount(0);
}

test('the owner makes an admin and removes them, hands ownership on, and the new owner reads all of it in the Audit log', async ({
	device,
	browser
}) => {
	const { server, page: owner } = device;
	const { library } = server.ids;
	const first = await createUser(server, [library]);
	const second = await createUser(server, [library]);
	const context = await browser.newContext({ baseURL: server.url });
	const other = await browser.newContext({ baseURL: server.url });
	try {
		// The owner promotes the first user, who makes an invite from their own browser.
		let row = await accountRow(owner, first.username);
		await row.getByRole('button', { name: 'Make admin' }).click();
		await confirm(owner, 'Make admin', server.password);
		await expect(row.getByText('Admin', { exact: true })).toBeVisible();

		const admin = await context.newPage();
		await login(admin, server.url, first.username, first.password);
		await admin.goto('/admin/invites');
		await admin.getByRole('button', { name: 'Create invite' }).click();
		await expect(admin.getByText('Share this link')).toBeVisible();

		// An admin who is not the owner is told why the Audit log is not theirs.
		await expect(admin.getByRole('link', { name: 'Audit log' })).toHaveCount(0);
		await admin.goto('/admin/audit');
		await expect(admin.getByRole('heading', { name: 'Admins only' })).toBeVisible();
		await expect(admin.getByText('Only the owner can read the audit log.')).toBeVisible();

		// Demoted, their invite shows as revoked to the owner.
		row = await accountRow(owner, first.username);
		await row.getByRole('button', { name: 'Remove admin' }).click();
		await confirm(owner, 'Remove admin', server.password);
		await owner.goto('/admin/invites');
		await expect(
			owner.getByRole('listitem').filter({ hasText: `Made by ${first.username}` })
		).toContainText('Revoked');

		// The owner hands ownership to the second user, once an admin.
		row = await accountRow(owner, second.username);
		await row.getByRole('button', { name: 'Make admin' }).click();
		await confirm(owner, 'Make admin', server.password);
		await expect(row.getByText('Admin', { exact: true })).toBeVisible();
		await row.getByRole('button', { name: 'Make owner' }).click();
		await confirm(owner, 'Make owner', server.password);
		await expect(row.getByRole('button', { name: 'Make owner' })).toHaveCount(0);

		// The new owner reads everything the test did.
		const next = await other.newPage();
		await login(next, server.url, second.username, second.password);
		await next.goto('/albums');
		await next.getByRole('link', { name: 'Audit log' }).click();
		await expect(next).toHaveURL(/\/admin\/audit$/);
		for (const text of [
			`${first.username} joined with an invite from owner`,
			`${second.username} joined with an invite from owner`,
			`owner made ${first.username} an admin`,
			`owner made ${first.username} a user`,
			`owner made ${second.username} an admin`,
			`owner handed ownership to ${second.username}`
		]) {
			await expect(next.getByText(text, { exact: true })).toBeVisible();
		}
		// Newest first.
		await expect(
			next.getByRole('region', { name: 'Entries' }).getByRole('listitem').first()
		).toContainText(`owner handed ownership to ${second.username}`);

		// The former owner is an admin now, with no Audit log.
		await owner.goto('/albums');
		await expect(owner.getByRole('link', { name: 'Accounts' })).toBeVisible();
		await expect(owner.getByRole('link', { name: 'Audit log' })).toHaveCount(0);
		await owner.goto('/account');
		await expect(owner.getByRole('main').getByRole('link', { name: 'Invites' })).toBeVisible();
		await expect(owner.getByRole('main').getByRole('link', { name: 'Audit log' })).toHaveCount(0);
	} finally {
		await context.close();
		await other.close();
	}
});
