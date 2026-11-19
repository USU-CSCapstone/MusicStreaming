import { page } from 'vitest/browser';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const navigation = vi.hoisted(() => ({ goto: vi.fn() }));
vi.mock('$app/navigation', () => navigation);
const catalog = vi.hoisted(() => ({ signOut: vi.fn() }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog }));

const { default: Invite } = await import('./+page.svelte');
const { connectivity } = await import('$lib/connectivity.svelte');

/** The server's answers, by path; what each request asked is in `sent`. */
let answers: Record<string, () => Response>;
let sent: { path: string; body?: unknown }[];

beforeEach(() => {
	answers = {
		'/api/v1/me': () => Response.json({ code: 'unauthenticated' }, { status: 401 }),
		'/api/v1/auth/invites/check': () => new Response(null, { status: 204 }),
		'/api/v1/auth/invites/redeem': () => Response.json({ id: 's1' }, { status: 201 })
	};
	sent = [];
	vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
		const path = new URL(String(input), location.href).pathname;
		sent.push({ path, body: init?.body ? JSON.parse(String(init.body)) : undefined });
		return answers[path]();
	});
	location.hash = 'secret-code';
});

afterEach(() => {
	localStorage.clear();
	connectivity.setWorkOffline(false);
	vi.restoreAllMocks();
	navigation.goto.mockClear();
	catalog.signOut.mockClear();
});

/** An answer that refuses with `status` and `code`, saying `detail`. */
const problem = (status: number, code: string, detail: string) => () =>
	Response.json({ status, code, title: 'Refused', detail }, { status });

it('shows the form to someone signed out, takes the code out of the address bar, and signs them in', async () => {
	history.replaceState({ kept: 1 }, '', location.href);
	render(Invite);
	await expect.element(page.getByLabelText('Username')).toBeVisible();
	expect(location.hash).toBe('');
	// SvelteKit's router state stays with the page.
	expect(history.state).toEqual({ kept: 1 });
	expect(sent.find((r) => r.path.endsWith('/check'))?.body).toEqual({ code: 'secret-code' });

	await page.getByLabelText('Username').fill('ann');
	await page.getByLabelText('Display name').fill('Ann');
	await page.getByLabelText('Password').fill('a long and good password');
	await page.getByRole('button', { name: 'Create account' }).click();

	await vi.waitFor(() =>
		expect(navigation.goto).toHaveBeenCalledWith('/', { invalidateAll: true })
	);
	expect(sent.find((r) => r.path.endsWith('/redeem'))?.body).toMatchObject({
		code: 'secret-code',
		username: 'ann',
		displayName: 'Ann',
		password: 'a long and good password',
		device: { name: expect.any(String) }
	});
});

it('shows the form on a device another account’s ended session left, and leaves its files', async () => {
	localStorage.setItem('jewelcase-owner', 'someone-else');
	render(Invite);
	await expect.element(page.getByLabelText('Username')).toBeVisible();
	expect(catalog.signOut).not.toHaveBeenCalled();
	expect(localStorage.getItem('jewelcase-owner')).toBe('someone-else');
});

it('says a dead link is no longer valid, with no form', async () => {
	answers['/api/v1/auth/invites/check'] = problem(404, 'not_found', 'Not found.');
	render(Invite);
	await expect.element(page.getByText('This link is no longer valid.')).toBeVisible();
	await expect.element(page.getByLabelText('Username')).not.toBeInTheDocument();
});

it('says a link with no code is no longer valid, without asking the server', async () => {
	location.hash = '';
	render(Invite);
	await expect.element(page.getByText('This link is no longer valid.')).toBeVisible();
	expect(sent.map((r) => r.path)).toEqual(['/api/v1/me']);
});

it('asks someone signed in to sign out first, and touches nothing of theirs', async () => {
	localStorage.setItem('jewelcase-owner', 'u1');
	answers['/api/v1/me'] = () => Response.json({ id: 'u1', username: 'bo', displayName: 'Bo' });
	render(Invite);
	await expect
		.element(page.getByText("You're signed in as Bo. Sign out first to use this invite."))
		.toBeVisible();
	await expect.element(page.getByLabelText('Username')).not.toBeInTheDocument();
	expect(location.hash).toBe('');
	expect(sent.map((r) => r.path)).toEqual(['/api/v1/me']);
	expect(catalog.signOut).not.toHaveBeenCalled();
	expect(localStorage.getItem('jewelcase-owner')).toBe('u1');
});

it.each([
	[409, 'username_taken', 'That username is taken.'],
	[422, 'weak_password', 'This password is too easy to guess.']
])('shows the server’s %i and keeps the form filled', async (status, code, detail) => {
	answers['/api/v1/auth/invites/redeem'] = problem(status, code, detail);
	render(Invite);
	await page.getByLabelText('Username').fill('ann');
	await page.getByLabelText('Password').fill('password');
	await page.getByRole('button', { name: 'Create account' }).click();

	await expect.element(page.getByRole('alert')).toHaveTextContent(detail);
	await expect.element(page.getByLabelText('Username')).toHaveValue('ann');
	await expect.element(page.getByLabelText('Password')).toHaveValue('password');
	expect(navigation.goto).not.toHaveBeenCalled();
});

it('turns Work offline off to check the link', async () => {
	connectivity.setWorkOffline(true);
	render(Invite);
	await expect.element(page.getByLabelText('Username')).toBeVisible();
	expect(connectivity.workOffline).toBe(false);
});
