import { ApiError } from '$lib/api/client';
import { page } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Invite, Library } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	created: [] as unknown[],
	fail: null as Error | null,
	code: 'secretcode',
	more: null as unknown,
	revoked: null as unknown
}));

vi.mock('$app/paths', () => ({
	base: '/jc',
	assets: '',
	asset: (file: string) => file,
	resolve: (path: string) => `/jc${path}`,
	resolveRoute: (path: string) => `/jc${path}`,
	match: async () => null
}));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<object>()),
	adminCreateInvite: async (_f: unknown, request: unknown) => {
		fake.created.push(request);
		if (fake.fail) throw fake.fail;
		return { ...invite('new', 'pending'), code: fake.code };
	},
	adminListInvites: async (_f: unknown, cursor?: string) =>
		cursor ? fake.more : { items: [invite('new', 'pending')], nextCursor: null, total: 1 },
	adminRevokeInvite: async () => fake.revoked
}));

const { default: Invites } = await import('./+page.svelte');

const libraries = [
	{ id: 'l1', name: 'Music' },
	{ id: 'l2', name: 'Podcasts' }
] as Library[];

/** An invite in `status`, made by Ada, for the first library. */
function invite(id: string, status: Invite['status'], more: Partial<Invite> = {}): Invite {
	return {
		id,
		status,
		createdBy: { id: 'a', username: 'ada' },
		redeemedBy: null,
		createdAt: '2026-11-01T10:00:00Z',
		expiresAt: '2026-11-08T10:00:00Z',
		libraryIds: ['l1'],
		...more
	};
}

const open = (items: Invite[], nextCursor: string | null = null) => ({
	data: { invites: { items, nextCursor, total: items.length }, libraries }
});

beforeEach(() => {
	fake.created = [];
	fake.fail = null;
	fake.code = 'secretcode';
});

it('creates an invite for the chosen libraries and shows its link once, with where it works', async () => {
	const { unmount } = render(Invites, open([]) as never);
	await page.getByLabelText('Podcasts').click();
	await page.getByRole('button', { name: 'Create invite' }).click();

	await expect.element(page.getByText(`${location.origin}/jc/invite#secretcode`)).toBeVisible();
	await expect.element(page.getByText(/works only where this address works/)).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Copy' })).toBeVisible();
	expect(fake.created).toEqual([{ libraryIds: ['l2'], expiresInDays: 7 }]);

	// Back on the page later, the server's list holds no code to show.
	unmount();
	render(Invites, open([invite('new', 'pending')]) as never);
	await expect.element(page.getByText('Pending')).toBeVisible();
	expect(document.body.textContent).not.toContain('secretcode');
});

it('labels each state, names who made and used it, and loads more', async () => {
	fake.more = {
		items: [invite('e', 'expired', { createdBy: null })],
		nextCursor: null,
		total: 4
	};
	render(
		Invites,
		open(
			[
				invite('p', 'pending'),
				invite('r', 'redeemed', {
					redeemedBy: { id: 'b', username: 'bob' },
					redeemedAt: '2026-11-02T10:00:00Z',
					libraryIds: ['l1', 'l2']
				}),
				invite('v', 'revoked', { libraryIds: [] })
			],
			'next'
		) as never
	);
	for (const label of ['Pending', 'Redeemed', 'Revoked']) {
		await expect.element(page.getByText(label, { exact: true })).toBeVisible();
	}
	await expect.element(page.getByText('used by bob')).toBeVisible();
	await expect.element(page.getByText('Libraries: Music, Podcasts')).toBeVisible();

	await page.getByRole('button', { name: 'Load more' }).click();
	await expect.element(page.getByText('Expired', { exact: true })).toBeVisible();
	await expect.element(page.getByText('Made by a removed account')).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
});

it('updates a row from the answer to a revoke, whatever it says', async () => {
	// Someone redeemed it just before the admin pressed Revoke.
	fake.revoked = invite('p', 'redeemed', {
		redeemedBy: { id: 'b', username: 'bob' },
		redeemedAt: '2026-11-02T10:00:00Z'
	});
	render(Invites, open([invite('p', 'pending')]) as never);
	await page.getByRole('button', { name: 'Revoke' }).click();
	await expect.element(page.getByText('Redeemed', { exact: true })).toBeVisible();
	await expect.element(page.getByText('used by bob')).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Revoke' })).not.toBeInTheDocument();
});

it("sends the chosen expiry, and shows the server's refusal in the form", async () => {
	fake.fail = new ApiError(400, 'invalid', 'Pick a shorter time.');
	render(Invites, open([]) as never);
	await page.getByLabelText('Days until it expires').fill('3');
	await page.getByRole('button', { name: 'Create invite' }).click();
	await expect.element(page.getByText('Pick a shorter time.')).toBeVisible();
	expect(fake.created).toEqual([{ libraryIds: [], expiresInDays: 3 }]);
});

it('does not say a new link was copied because the last one was', async () => {
	Object.defineProperty(navigator, 'clipboard', {
		value: { writeText: async () => {} },
		configurable: true
	});
	render(Invites, open([]) as never);
	await page.getByRole('button', { name: 'Create invite' }).click();
	await page.getByRole('button', { name: 'Copy' }).click();
	await expect.element(page.getByText('Copied.')).toBeVisible();

	fake.code = 'othercode';
	await page.getByRole('button', { name: 'Create invite' }).click();
	await expect.element(page.getByText(/invite#othercode/)).toBeVisible();
	await expect.element(page.getByText('Copied.')).not.toBeInTheDocument();
});
