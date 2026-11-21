import { randomUUID } from 'node:crypto';
import type { BrowserContext, Page } from '@playwright/test';
import { expect, test as base } from './test';

const test = base.extend<{
	/** A page in a browser that has never seen the server, as someone sent a link has; closed after the test. */
	stranger: () => Promise<Page>;
}>({
	stranger: async ({ browser, server }, use) => {
		const contexts: BrowserContext[] = [];
		try {
			await use(async () => {
				const context = await browser.newContext({ baseURL: server.url });
				contexts.push(context);
				return context.newPage();
			});
		} finally {
			await Promise.all(contexts.map((c) => c.close()));
		}
	}
});

/** Opens the invite link on `page` and creates an account with it. */
async function join(page: Page, link: string) {
	await page.goto(link);
	await expect(page.getByLabel('Username')).toBeVisible();
	expect(page.url()).not.toContain('#');
	await expect(page.getByRole('navigation', { name: 'Library' })).toHaveCount(0);
	await page.getByLabel('Username').fill(`e2e-${randomUUID().slice(0, 8)}`);
	await page.getByLabel('Password').fill(`e2e-${randomUUID()}`);
	await page.getByRole('button', { name: 'Create account' }).click();
}

test('someone invited to the library joins with the link and sees its albums, and the link is then used up', async ({
	page,
	stranger
}) => {
	await page.goto('/admin/invites');
	await page.getByRole('group', { name: 'Libraries they can reach' }).getByRole('checkbox').check();
	await page.getByRole('button', { name: 'Create invite' }).click();
	const link = await page.getByText(/\/invite#/).textContent();

	const invitee = await stranger();
	await join(invitee, link!);
	await expect(invitee).toHaveURL(/\/albums$/);
	await expect(invitee.getByText('Signal', { exact: true }).first()).toBeVisible();
	await expect(invitee.getByText('Hearth', { exact: true }).first()).toBeVisible();

	const late = await stranger();
	await late.goto(link!);
	await expect(late.getByText('This link is no longer valid.')).toBeVisible();
	await expect(late.getByLabel('Username')).toHaveCount(0);
});

test('someone invited to no library joins and is told they have no access yet', async ({
	server,
	stranger
}) => {
	const { code } = await server.api('/admin/invites', { method: 'POST', body: { libraryIds: [] } });
	const invitee = await stranger();
	await join(invitee, `/invite#${code}`);
	await expect(
		invitee.getByText("You don't have access to a library on this server yet.")
	).toBeVisible();
});
