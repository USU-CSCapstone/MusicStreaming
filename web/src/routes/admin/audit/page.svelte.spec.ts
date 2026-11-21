import { page, userEvent } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AuditEntry, Library } from '$lib/api/types';

const fake = vi.hoisted(() => ({ more: null as unknown, asked: [] as unknown[] }));

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
	adminListAuditLog: async (_f: unknown, query: { cursor?: string } = {}) => {
		fake.asked.push(query);
		return fake.more;
	}
}));

const { default: Audit } = await import('./+page.svelte');

const libraries = [
	{ id: '11', name: 'Music' },
	{ id: '12', name: 'Audiobooks' }
] as Library[];

const sam = { id: '1', username: 'sam', deleted: false };
const alex = { id: '2', username: 'alex', deleted: false };

let next = 100;
/** An entry as the server sends it: IDs as decimal strings, `detail` by action. */
function entry(
	action: AuditEntry['action'],
	more: Partial<AuditEntry> & { detail?: AuditEntry['detail'] } = {}
): AuditEntry {
	return {
		id: String(next--),
		at: '2026-11-01T10:00:00Z',
		action,
		actor: sam,
		target: alex,
		detail: {},
		...more
	};
}

const open = (items: AuditEntry[], nextCursor: string | null = null) => ({
	data: { audit: { items, nextCursor, total: items.length }, libraries }
});

beforeEach(() => {
	fake.asked = [];
	next = 100;
});

it('reads each of the ten actions as a sentence naming who did what to whom', async () => {
	render(
		Audit,
		open([
			entry('account_created', { detail: { inviteId: '5', libraryIds: ['11'] } }),
			entry('access_changed', { detail: { granted: ['11'], revoked: ['12'] } }),
			entry('suspended'),
			entry('restored'),
			entry('devices_signed_out', { detail: { deviceId: '9', deviceName: 'Pixel' } }),
			entry('devices_signed_out', { detail: { all: true } }),
			entry('password_reset_issued'),
			entry('password_reset_issued', { actor: null }),
			entry('password_reset_used', { actor: null }),
			entry('role_changed', { detail: { from: 'user', to: 'admin' } }),
			entry('role_changed', {
				target: { id: '3', username: 'kim', deleted: false },
				detail: { from: 'admin', to: 'user' }
			}),
			entry('ownership_transferred'),
			entry('deleted', { target: { ...alex, deleted: true } })
		]) as never
	);
	for (const text of [
		'alex joined with an invite from sam',
		'sam gave alex access to Music and took away Audiobooks',
		'sam suspended alex',
		'sam restored alex',
		'sam signed alex out of Pixel',
		'sam signed alex out of every device',
		'sam made a password reset link for alex',
		'a reset link for alex was made on the server',
		'someone used a reset link for alex',
		'sam made alex an admin',
		'sam made kim a user',
		'sam handed ownership to alex',
		'sam deleted alex'
	]) {
		await expect.element(page.getByText(text, { exact: true })).toBeVisible();
	}
});

it('names only what changed in access, and a library it does not know as "a library"', async () => {
	render(
		Audit,
		open([
			entry('access_changed', { detail: { granted: ['11', '12'], revoked: [] } }),
			entry('access_changed', { detail: { granted: [], revoked: ['99'] } })
		]) as never
	);
	await expect
		.element(page.getByText('sam gave alex access to Music and Audiobooks', { exact: true }))
		.toBeVisible();
	await expect
		.element(page.getByText("sam took away alex's access to a library", { exact: true }))
		.toBeVisible();
});

it('marks a deleted actor and a deleted target, and names an actor with no name', async () => {
	render(
		Audit,
		open([
			entry('suspended', {
				actor: { ...sam, deleted: true },
				target: { ...alex, deleted: true }
			}),
			entry('restored', { actor: { id: '3', username: null, deleted: true } })
		]) as never
	);
	await expect
		.element(page.getByText('sam (deleted) suspended alex (deleted)', { exact: true }))
		.toBeVisible();
	await expect
		.element(page.getByText('a deleted account restored alex', { exact: true }))
		.toBeVisible();
});

it('shows when each happened, and loads more of the older ones', async () => {
	fake.more = {
		items: [entry('suspended', { at: '2026-10-01T10:00:00Z', target: sam, actor: alex })],
		nextCursor: null,
		total: 2
	};
	render(Audit, open([entry('restored')], 'next') as never);
	const when = (iso: string) => new Date(iso).toLocaleString();
	await expect.element(page.getByText(when('2026-11-01T10:00:00Z'))).toBeVisible();

	await page.getByRole('button', { name: 'Load more' }).click();
	await expect.element(page.getByText('alex suspended sam', { exact: true })).toBeVisible();
	await expect.element(page.getByText(when('2026-10-01T10:00:00Z'))).toBeVisible();
	expect(fake.asked).toEqual([{ cursor: 'next' }]);
	await expect.element(page.getByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
	// The first page stays above the older one.
	const items = page.getByRole('listitem').elements();
	expect(items[0].textContent).toContain('sam restored alex');
});

it('says when nothing has been recorded', async () => {
	render(Audit, open([]) as never);
	await expect.element(page.getByText('Nothing has been recorded yet.')).toBeVisible();
});

it('fits a phone, and Load more is reached and pressed by keyboard', async () => {
	fake.more = { items: [entry('suspended')], nextCursor: null, total: 2 };
	await page.viewport(375, 700);
	render(
		Audit,
		open(
			[
				entry('access_changed', {
					actor: { ...sam, username: 'a-very-long-username-for-a-phone' },
					target: { ...alex, username: 'another-very-long-username-too' },
					detail: { granted: ['11', '12'], revoked: [] }
				})
			],
			'next'
		) as never
	);
	expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(375);
	await userEvent.tab();
	await expect.element(page.getByRole('button', { name: 'Load more' })).toHaveFocus();
	await userEvent.keyboard('{Enter}');
	await expect.element(page.getByText('sam suspended alex', { exact: true })).toBeVisible();
});
