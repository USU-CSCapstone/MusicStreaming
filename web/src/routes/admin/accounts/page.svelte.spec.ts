import { ApiError } from '$lib/api/client';
import { page, userEvent } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AdminUser, User } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	more: null as unknown,
	asked: [] as unknown[],
	set: [] as [string, string][],
	answer: null as unknown,
	fail: null as Error | null,
	went: [] as string[]
}));

vi.mock('$app/paths', () => ({
	base: '/jc',
	assets: '',
	asset: (file: string) => file,
	resolve: (path: string) => `/jc${path}`,
	resolveRoute: (path: string) => `/jc${path}`,
	match: async () => null
}));
vi.mock('$app/navigation', () => ({
	goto: async (url: string) => {
		fake.went.push(url);
	},
	invalidate: async () => {},
	invalidateAll: async () => {},
	beforeNavigate: () => {},
	afterNavigate: () => {},
	onNavigate: () => {}
}));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<object>()),
	adminListUsers: async (_f: unknown, query: unknown) => {
		fake.asked.push(query);
		return fake.more;
	},
	adminSetUserStatus: async (_f: unknown, id: string, status: string) => {
		fake.set.push([id, status]);
		if (fake.fail) throw fake.fail;
		return fake.answer;
	}
}));

const { default: Accounts } = await import('./+page.svelte');
const { load } = await import('./+page');

/** An active user named `name`, unless `more` says otherwise. */
function account(id: string, name: string, more: Partial<AdminUser> = {}): AdminUser {
	return {
		id,
		username: name,
		displayName: `${name} Display`,
		role: 'user',
		hasAvatar: false,
		status: 'active',
		createdAt: '2026-11-01T10:00:00Z',
		libraryIds: [],
		resetPending: false,
		...more
	};
}

const as = (role: User['role']) =>
	({ id: 'me', username: 'me', displayName: 'Me', role, hasAvatar: false }) as User;

const open = (items: AdminUser[], me: User, q = '', nextCursor: string | null = null) => ({
	data: { users: { items, nextCursor, total: items.length }, me, q }
});

const buttonsOn = (name: string) =>
	[...document.querySelectorAll('li')]
		.find((li) => li.textContent?.includes(name))
		?.querySelectorAll('button').length;

beforeEach(() => {
	fake.set = [];
	fake.asked = [];
	fake.fail = null;
	fake.went = [];
});

it('lists accounts with their role, suspension and pending reset, and pages on Load more', async () => {
	fake.more = { items: [account('9', 'zed')], nextCursor: null, total: 4 };
	render(
		Accounts,
		open(
			[
				account('2', 'ada', { role: 'admin' }),
				account('3', 'bob', { status: 'suspended' }),
				account('4', 'cy', { resetPending: true })
			],
			as('owner'),
			'bo',
			'next'
		) as never
	);
	await expect.element(page.getByText('ada Display')).toBeVisible();
	await expect.element(page.getByText('Admin', { exact: true })).toBeVisible();
	await expect.element(page.getByText('Suspended', { exact: true })).toBeVisible();
	await expect.element(page.getByText('Reset pending')).toBeVisible();

	await page.getByRole('button', { name: 'Load more' }).click();
	await expect.element(page.getByText('zed Display')).toBeVisible();
	expect(fake.asked).toEqual([{ q: 'bo', cursor: 'next' }]);
	await expect.element(page.getByText('ada Display')).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
});

it('puts a search in the address, and a new search replaces the rows of the last', async () => {
	const view = render(Accounts, open([account('2', 'ada')], as('owner')) as never);
	await page.getByRole('searchbox', { name: 'Search accounts' }).fill('bo b');
	await page.getByRole('button', { name: 'Search' }).click();
	expect(fake.went).toEqual(['/jc/admin/accounts?q=bo+b']);

	// The load that the new address ran hands the page its first page.
	await view.rerender(open([account('3', 'bob')], as('owner'), 'bo b') as never);
	await expect.element(page.getByText('bob Display')).toBeVisible();
	await expect.element(page.getByText('ada Display')).not.toBeInTheDocument();
	await expect
		.element(page.getByRole('searchbox', { name: 'Search accounts' }))
		.toHaveValue('bo b');
});

it('offers actions only on accounts the viewer outranks', async () => {
	const rows = [
		account('1', 'olga', { role: 'owner' }),
		account('2', 'ada', { role: 'admin' }),
		account('me', 'me', { role: 'admin' }),
		account('3', 'bob')
	];
	const admin = render(Accounts, open(rows, as('admin')) as never);
	await expect.element(page.getByText('bob Display')).toBeVisible();
	expect(buttonsOn('olga')).toBe(0);
	expect(buttonsOn('ada')).toBe(0);
	expect(buttonsOn('me Display')).toBe(0);
	expect(buttonsOn('bob')).toBe(1);
	await admin.unmount();

	render(Accounts, open([...rows.slice(0, 1), ...rows.slice(1)], as('owner')) as never);
	await expect.element(page.getByText('bob Display')).toBeVisible();
	expect(buttonsOn('olga')).toBe(0);
	expect(buttonsOn('ada')).toBe(1);
	expect(buttonsOn('bob')).toBe(1);
});

it('offers no action to a user', async () => {
	render(Accounts, open([account('3', 'bob')], as('user')) as never);
	await expect.element(page.getByText('bob Display')).toBeVisible();
	expect(buttonsOn('bob')).toBe(0);
});

it('asks before suspending, then shows the account as the server now has it', async () => {
	fake.answer = account('3', 'bob', { status: 'suspended' });
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Suspend' }).click();
	await expect.element(page.getByRole('dialog', { name: 'Suspend bob?' })).toBeVisible();
	expect(fake.set).toEqual([]);

	await page.getByRole('dialog').getByRole('button', { name: 'Suspend' }).click();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(fake.set).toEqual([['3', 'suspended']]);
	await expect.element(page.getByText('Suspended', { exact: true })).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Restore' })).toBeVisible();
});

it('restores a suspended account at once', async () => {
	fake.answer = account('3', 'bob');
	render(Accounts, open([account('3', 'bob', { status: 'suspended' })], as('admin')) as never);
	await page.getByRole('button', { name: 'Restore' }).click();
	await expect.element(page.getByRole('button', { name: 'Suspend' })).toBeVisible();
	expect(fake.set).toEqual([['3', 'active']]);
	await expect.element(page.getByText('Suspended', { exact: true })).not.toBeInTheDocument();
});

it('shows the server’s reason in the dialog when it refuses', async () => {
	fake.fail = new ApiError(403, 'forbidden', 'Only the owner can do this to an admin’s account.');
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Suspend' }).click();
	await page.getByRole('dialog').getByRole('button', { name: 'Suspend' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
	await expect.element(page.getByRole('dialog')).toBeVisible();
});

it('lets Escape close the dialog without suspending', async () => {
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Suspend' }).click();
	await userEvent.keyboard('{Escape}');
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(fake.set).toEqual([]);
});

it('reads the first page of a search from the server', async () => {
	fake.more = { items: [], nextCursor: null, total: 0 };
	const url = new URL('http://x/admin/accounts?q=bo');
	const loaded = await load({ fetch, url } as never);
	expect(fake.asked).toEqual([{ q: 'bo' }]);
	expect(loaded).toMatchObject({ q: 'bo' });
});

it('shows why a refused Restore failed, on the account’s row', async () => {
	fake.fail = new ApiError(403, 'forbidden', 'Only the owner can do this to an admin’s account.');
	render(Accounts, open([account('3', 'bob', { status: 'suspended' })], as('admin')) as never);
	await page.getByRole('button', { name: 'Restore' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
	await expect.element(page.getByRole('button', { name: 'Restore' })).toBeEnabled();
});
