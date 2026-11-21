import { ApiError } from '$lib/api/client';
import { page, userEvent } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AdminUser, Device, Library, User } from '$lib/api/types';

const LIBRARIES = vi.hoisted(
	() =>
		[
			{ id: 'lib-a', name: 'Alpha' },
			{ id: 'lib-b', name: 'Beta' }
		] as Library[]
);

const fake = vi.hoisted(() => ({
	more: null as unknown,
	asked: [] as unknown[],
	set: [] as [string, string][],
	issued: [] as [string, string][],
	deleted: [] as [string, string][],
	answer: null as unknown,
	fail: null as Error | null,
	went: [] as string[],
	libraries: [] as [string, string[]][],
	changes: [] as unknown[],
	devices: [] as Device[],
	devicesAsked: [] as string[],
	out: [] as [string, string][],
	outAll: [] as string[],
	failDevices: null as Error | null
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
	adminIssuePasswordReset: async (_f: unknown, id: string, body: { password: string }) => {
		fake.issued.push([id, body.password]);
		if (fake.fail) throw fake.fail;
		return { code: 'reset-code', expiresAt: '2026-11-21T10:00:00Z' };
	},
	adminDeleteUser: async (_f: unknown, id: string, body: { password: string }) => {
		fake.deleted.push([id, body.password]);
		if (fake.fail) throw fake.fail;
	},
	adminSetUserStatus: async (_f: unknown, id: string, status: string) => {
		fake.set.push([id, status]);
		if (fake.fail) throw fake.fail;
		return fake.answer;
	},
	listLibraries: async () => ({ items: LIBRARIES }),
	adminSetUserLibraries: async (_f: unknown, id: string, libraryIds: string[]) => {
		fake.libraries.push([id, libraryIds]);
		if (fake.fail) throw fake.fail;
		return fake.answer;
	},
	adminChangeLibraryAccess: async (_f: unknown, change: unknown) => {
		fake.changes.push(change);
		if (fake.fail) throw fake.fail;
	},
	adminListUserDevices: async (_f: unknown, id: string) => {
		fake.devicesAsked.push(id);
		if (fake.failDevices) throw fake.failDevices;
		return { items: fake.devices };
	},
	adminSignOutDevice: async (_f: unknown, user: string, device: string) => {
		fake.out.push([user, device]);
		if (fake.fail) throw fake.fail;
		fake.devices = fake.devices.filter((d) => d.id !== device);
	},
	adminSignOutDevices: async (_f: unknown, user: string) => {
		fake.outAll.push(user);
		if (fake.fail) throw fake.fail;
		fake.devices = [];
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
	data: { users: { items, nextCursor, total: items.length }, me, q, libraries: LIBRARIES }
});

/** A signed-in device named `name`. */
const device = (id: string, name: string, connected = false) =>
	({
		id,
		name,
		type: 'desktop',
		firstSeenAt: '2026-11-01T10:00:00Z',
		lastSeenAt: '2026-11-02T10:00:00Z',
		connected
	}) as Device;

const rowOf = (name: string) => page.getByRole('listitem').filter({ hasText: `${name} Display` });

const buttonsOn = (name: string) =>
	[...document.querySelectorAll('li')]
		.find((li) => li.textContent?.includes(name))
		?.querySelectorAll('button').length;

beforeEach(() => {
	fake.set = [];
	fake.issued = [];
	fake.deleted = [];
	fake.asked = [];
	fake.fail = null;
	fake.went = [];
	fake.libraries = [];
	fake.changes = [];
	fake.devices = [];
	fake.devicesAsked = [];
	fake.out = [];
	fake.outAll = [];
	fake.failDevices = null;
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
	expect(buttonsOn('bob')).toBe(5);
	await admin.unmount();

	render(Accounts, open([...rows.slice(0, 1), ...rows.slice(1)], as('owner')) as never);
	await expect.element(page.getByText('bob Display')).toBeVisible();
	expect(buttonsOn('olga')).toBe(0);
	expect(buttonsOn('ada')).toBe(5);
	expect(buttonsOn('bob')).toBe(5);
});

it('asks for the admin’s password, then shows the reset link once and the account as having a reset pending', async () => {
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Reset password' }).click();
	const dialog = page.getByRole('dialog', { name: 'Reset bob’s password?' });
	await expect.element(dialog).toBeVisible();
	expect(fake.issued).toEqual([]);

	await dialog.getByLabelText('Your password').fill('my own password');
	await dialog.getByRole('button', { name: 'Reset password' }).click();

	await expect.element(page.getByText(`${location.origin}/jc/reset#reset-code`)).toBeVisible();
	expect(fake.issued).toEqual([['3', 'my own password']]);
	await expect.element(page.getByText('Reset pending')).toBeVisible();
	await expect.element(page.getByText(/24 hours/)).toBeVisible();
	await expect.element(page.getByText(/works only where this address works/)).toBeVisible();
	await expect.element(page.getByText(/replaces/)).toBeVisible();

	// Dismissed, the link is gone and is not shown again.
	await page.getByRole('button', { name: 'Done' }).click();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(document.body.textContent).not.toContain('reset-code');
});

it('shows the server’s reason when the password is wrong, and keeps the dialog for another try', async () => {
	fake.fail = new ApiError(403, 'wrong_password', 'Your current password is not correct.');
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Reset password' }).click();
	const dialog = page.getByRole('dialog');
	await dialog.getByLabelText('Your password').fill('wrong');
	await dialog.getByRole('button', { name: 'Reset password' }).click();
	await expect
		.element(page.getByRole('alert'))
		.toHaveTextContent('Your current password is not correct.');
	await expect.element(dialog).toBeVisible();
	expect(document.body.textContent).not.toContain('reset-code');
	await expect.element(page.getByText('Reset pending')).not.toBeInTheDocument();
});

it('names what deleting an account takes and leaves, and sends the admin’s password', async () => {
	render(Accounts, open([account('3', 'bob'), account('4', 'cy')], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Delete' }).click();
	const dialog = page.getByRole('dialog', { name: 'Delete bob?' });
	await expect.element(dialog).toBeVisible();
	await expect.element(dialog).toHaveTextContent(/playlists, listening history, likes and devices/);
	await expect.element(dialog).toHaveTextContent(/No music files are touched/);
	await expect.element(dialog).toHaveTextContent(/Plugins bob connected keep what they already/);
	await expect.element(dialog).toHaveTextContent(/cannot be undone/);
	expect(fake.deleted).toEqual([]);

	await dialog.getByLabelText('Your password').fill('my own password');
	await dialog.getByRole('button', { name: 'Delete' }).click();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(fake.deleted).toEqual([['3', 'my own password']]);
});

it('drops a deleted account from the list and the selection, keeps the other rows including a second page, and closes its panel', async () => {
	fake.more = { items: [account('9', 'zed')], nextCursor: null, total: 4 };
	render(
		Accounts,
		open([account('3', 'bob'), account('4', 'cy')], as('admin'), '', 'next') as never
	);
	await page.getByRole('button', { name: 'Load more' }).click();
	await expect.element(page.getByText('zed Display')).toBeVisible();
	await page.getByRole('checkbox', { name: 'Select bob' }).click();
	await page.getByRole('checkbox', { name: 'Select zed' }).click();
	await expect.element(page.getByText('2 selected')).toBeVisible();
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await expect.element(page.getByRole('button', { name: 'Devices', expanded: true })).toBeVisible();

	await rowOf('bob').getByRole('button', { name: 'Delete' }).click();
	await page.getByLabelText('Your password').fill('pw');
	await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();

	await expect.element(page.getByText('bob Display')).not.toBeInTheDocument();
	await expect.element(page.getByText('cy Display')).toBeVisible();
	await expect.element(page.getByText('zed Display')).toBeVisible();
	await expect.element(page.getByText('1 selected')).toBeVisible();
	await expect.element(page.getByRole('checkbox', { name: 'Select zed' })).toBeChecked();
	// Nothing was read again: the list kept its pages.
	expect(fake.asked).toEqual([{ q: '', cursor: 'next' }]);
});

it('drops a reset link shown for an account once that account is deleted, which cancelled it', async () => {
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Reset password' }).click();
	await page.getByLabelText('Your password').fill('pw');
	await page.getByRole('dialog').getByRole('button', { name: 'Reset password' }).click();
	await expect.element(page.getByText(`${location.origin}/jc/reset#reset-code`)).toBeVisible();

	await rowOf('bob').getByRole('button', { name: 'Delete' }).click();
	await page.getByLabelText('Your password').fill('pw');
	await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
	await expect.element(page.getByText('bob Display')).not.toBeInTheDocument();
	expect(document.body.textContent).not.toContain('reset-code');
});

it('keeps the dialog and shows the server’s reason when the password is wrong, or the delete is refused', async () => {
	fake.fail = new ApiError(403, 'wrong_password', 'Your current password is not correct.');
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('button', { name: 'Delete' }).click();
	const dialog = page.getByRole('dialog');
	await dialog.getByLabelText('Your password').fill('wrong');
	await dialog.getByRole('button', { name: 'Delete' }).click();
	await expect
		.element(page.getByRole('alert'))
		.toHaveTextContent('Your current password is not correct.');
	await expect.element(dialog).toBeVisible();
	await expect.element(page.getByText('bob Display')).toBeVisible();

	fake.fail = new ApiError(404, 'not_found', 'Not Found');
	await dialog.getByRole('button', { name: 'Delete' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent('Not Found');
	await expect.element(dialog).toBeVisible();
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
	expect(loaded).toMatchObject({ q: 'bo', libraries: LIBRARIES });
});

it('shows why a refused Restore failed, on the account’s row', async () => {
	fake.fail = new ApiError(403, 'forbidden', 'Only the owner can do this to an admin’s account.');
	render(Accounts, open([account('3', 'bob', { status: 'suspended' })], as('admin')) as never);
	await page.getByRole('button', { name: 'Restore' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
	await expect.element(page.getByRole('button', { name: 'Restore' })).toBeEnabled();
});

it('sets one account’s libraries from the checkboxes, and shows the row the server answers', async () => {
	fake.answer = account('3', 'bob', { libraryIds: ['lib-b'] });
	render(Accounts, open([account('3', 'bob', { libraryIds: ['lib-a'] })], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Libraries' }).click();
	const alpha = page.getByRole('checkbox', { name: 'Alpha' });
	const beta = page.getByRole('checkbox', { name: 'Beta' });
	await expect.element(alpha).toBeChecked();
	await expect.element(beta).not.toBeChecked();

	await alpha.click();
	await beta.click();
	expect(fake.libraries).toEqual([]);
	await page.getByRole('button', { name: 'Save libraries' }).click();
	await vi.waitFor(() => expect(fake.libraries).toEqual([['3', ['lib-b']]]));
	await expect.element(page.getByRole('checkbox', { name: 'Beta' })).toBeChecked();
	await expect.element(page.getByRole('checkbox', { name: 'Alpha' })).not.toBeChecked();
});

it('shows the server’s reason when setting libraries is refused', async () => {
	fake.fail = new ApiError(
		422,
		'validation_failed',
		'libraryIds names a library that does not exist'
	);
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Libraries' }).click();
	await page.getByRole('button', { name: 'Save libraries' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/names a library that does not/);
});

it('grants a library to the selected accounts, then reads the rows again and clears the selection', async () => {
	render(
		Accounts,
		open(
			[
				account('2', 'ada', { role: 'admin' }),
				account('3', 'bob'),
				account('4', 'cy'),
				account('5', 'di')
			],
			as('admin')
		) as never
	);
	// An admin has no checkbox on an admin's row.
	await expect.element(page.getByRole('checkbox', { name: 'Select ada' })).not.toBeInTheDocument();
	await expect
		.element(page.getByRole('combobox', { name: 'Library for the selection' }))
		.not.toBeInTheDocument();

	await page.getByRole('checkbox', { name: 'Select bob' }).click();
	await page.getByRole('checkbox', { name: 'Select di' }).click();
	await expect.element(page.getByText('2 selected')).toBeVisible();
	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-b');

	fake.more = {
		items: [account('3', 'bob', { libraryIds: ['lib-b'] }), account('4', 'cy')],
		nextCursor: null,
		total: 2
	};
	await page.getByRole('button', { name: 'Grant' }).click();
	await vi.waitFor(() =>
		expect(fake.changes).toEqual([{ userIds: ['3', '5'], libraryIds: ['lib-b'], action: 'grant' }])
	);
	// The selected rows are read again: the list is read from its first page.
	await vi.waitFor(() => expect(fake.asked).toEqual([{ q: '' }]));
	await expect.element(page.getByText('selected')).not.toBeInTheDocument();
	await expect.element(page.getByRole('checkbox', { name: 'Select bob' })).not.toBeChecked();
});

it('revokes a library for the selected accounts', async () => {
	fake.more = { items: [account('3', 'bob')], nextCursor: null, total: 1 };
	render(Accounts, open([account('3', 'bob', { libraryIds: ['lib-a'] })], as('owner')) as never);
	await page.getByRole('checkbox', { name: 'Select bob' }).click();
	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-a');
	await page.getByRole('button', { name: 'Revoke' }).click();
	await vi.waitFor(() =>
		expect(fake.changes).toEqual([{ userIds: ['3'], libraryIds: ['lib-a'], action: 'revoke' }])
	);
});

it('asks for a library before granting or revoking, and keeps the selection when the server refuses', async () => {
	fake.fail = new ApiError(403, 'forbidden', 'Only the owner can do this to an admin’s account.');
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await page.getByRole('checkbox', { name: 'Select bob' }).click();
	await expect.element(page.getByRole('button', { name: 'Grant' })).toBeDisabled();
	await expect.element(page.getByRole('button', { name: 'Revoke' })).toBeDisabled();

	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-a');
	await page.getByRole('button', { name: 'Grant' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
	await expect.element(page.getByRole('checkbox', { name: 'Select bob' })).toBeChecked();
	await expect.element(page.getByText('1 selected')).toBeVisible();
});

it('keeps the selection when the list is loaded again, and acts only on what was chosen', async () => {
	const rows = [account('3', 'bob'), account('4', 'cy')];
	const view = render(Accounts, open(rows, as('admin')) as never);
	await page.getByRole('checkbox', { name: 'Select cy' }).click();
	await view.rerender(open(rows, as('admin')) as never);
	await expect.element(page.getByRole('checkbox', { name: 'Select cy' })).toBeChecked();
	await expect.element(page.getByRole('checkbox', { name: 'Select bob' })).not.toBeChecked();
	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-a');
	fake.more = { items: rows, nextCursor: null, total: 2 };
	await page.getByRole('button', { name: 'Grant' }).click();
	await vi.waitFor(() =>
		expect(fake.changes).toEqual([{ userIds: ['4'], libraryIds: ['lib-a'], action: 'grant' }])
	);
});

it('does not send a selection of more than 1,000 accounts', async () => {
	const rows = Array.from({ length: 1001 }, (_, i) => account(`u${i}`, `user${i}`));
	render(Accounts, open(rows, as('admin')) as never);
	const boxes = document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
	for (const box of boxes) box.click();
	await expect.element(page.getByText('1001 selected')).toBeVisible();
	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-a');
	await expect.element(page.getByRole('button', { name: 'Grant' })).toBeDisabled();
	await expect.element(page.getByText(/at most 1,000/)).toBeVisible();
});

it('lists an account’s devices when they are opened, not before', async () => {
	fake.devices = [device('d1', 'Kitchen laptop', true), device('d2', 'Old phone')];
	render(Accounts, open([account('3', 'bob'), account('4', 'cy')], as('admin')) as never);
	expect(fake.devicesAsked).toEqual([]);
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await expect.element(page.getByText('Kitchen laptop')).toBeVisible();
	await expect.element(page.getByText('Old phone')).toBeVisible();
	expect(fake.devicesAsked).toEqual(['3']);
});

it('signs out one device and reads the list again', async () => {
	fake.devices = [device('d1', 'Kitchen laptop', true), device('d2', 'Old phone')];
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await page.getByRole('button', { name: 'Sign out Old phone' }).click();
	await expect.element(page.getByText('Old phone')).not.toBeInTheDocument();
	expect(fake.out).toEqual([['3', 'd2']]);
	expect(fake.devicesAsked).toEqual(['3', '3']);
	await expect.element(page.getByText('Kitchen laptop')).toBeVisible();
});

it('signs out every device only after asking', async () => {
	fake.devices = [device('d1', 'Kitchen laptop', true), device('d2', 'Old phone')];
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await page.getByRole('button', { name: 'Sign out all' }).click();
	await expect
		.element(page.getByRole('dialog', { name: 'Sign out bob everywhere?' }))
		.toBeVisible();
	expect(fake.outAll).toEqual([]);

	await page.getByRole('dialog').getByRole('button', { name: 'Sign out all' }).click();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(fake.outAll).toEqual(['3']);
	await expect.element(page.getByText('No signed-in devices.')).toBeVisible();
	expect(fake.devicesAsked).toEqual(['3', '3']);
});

it('shows the server’s reason when the devices cannot be read or signed out', async () => {
	fake.failDevices = new ApiError(
		403,
		'forbidden',
		'Only the owner can do this to an admin’s account.'
	);
	render(Accounts, open([account('3', 'bob')], as('owner')) as never);
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
});

it('shows why a refused device sign-out failed, and keeps the device listed', async () => {
	fake.devices = [device('d1', 'Kitchen laptop')];
	fake.fail = new ApiError(404, undefined, 'Not Found');
	render(Accounts, open([account('3', 'bob')], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Devices' }).click();
	await page.getByRole('button', { name: 'Sign out Kitchen laptop' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/^Not Found$/);
	await expect.element(page.getByText('Kitchen laptop')).toBeVisible();
});

it('shows the account as the server answers a save, not as it was ticked', async () => {
	fake.answer = account('3', 'bob', { libraryIds: ['lib-b'] });
	render(Accounts, open([account('3', 'bob', { libraryIds: ['lib-a'] })], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Libraries' }).click();
	await page.getByRole('checkbox', { name: 'Beta' }).click();
	await page.getByRole('button', { name: 'Save libraries' }).click();
	await vi.waitFor(() => expect(fake.libraries).toEqual([['3', ['lib-a', 'lib-b']]]));
	await expect.element(page.getByRole('checkbox', { name: 'Alpha' })).not.toBeChecked();
	await expect.element(page.getByRole('checkbox', { name: 'Beta' })).toBeChecked();
});

it('shows the grants a bulk change just made in an open Libraries panel, so Save keeps them', async () => {
	fake.answer = account('3', 'bob', { libraryIds: ['lib-a', 'lib-b'] });
	render(Accounts, open([account('3', 'bob', { libraryIds: ['lib-a'] })], as('admin')) as never);
	await rowOf('bob').getByRole('button', { name: 'Libraries' }).click();
	await page.getByRole('checkbox', { name: 'Select bob' }).click();
	await page.getByRole('combobox', { name: 'Library for the selection' }).selectOptions('lib-b');
	fake.more = {
		items: [account('3', 'bob', { libraryIds: ['lib-a', 'lib-b'] })],
		nextCursor: null,
		total: 1
	};
	await page.getByRole('button', { name: 'Grant' }).click();
	await vi.waitFor(() => expect(fake.asked).toEqual([{ q: '' }]));
	await expect.element(page.getByRole('checkbox', { name: 'Beta' })).toBeChecked();
	await page.getByRole('button', { name: 'Save libraries' }).click();
	await vi.waitFor(() => expect(fake.libraries).toEqual([['3', ['lib-a', 'lib-b']]]));
});
