import { page } from 'vitest/browser';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const navigation = vi.hoisted(() => ({ goto: vi.fn() }));
vi.mock('$app/navigation', () => navigation);

const { default: Reset } = await import('./+page.svelte');
const { connectivity } = await import('$lib/connectivity.svelte');

/** The server's answers, by path; what each request asked is in `sent`. */
let answers: Record<string, () => Response>;
let sent: { path: string; body?: unknown }[];

beforeEach(() => {
	answers = {
		'/api/v1/me': () => Response.json({ code: 'unauthenticated' }, { status: 401 }),
		'/api/v1/auth/password-resets/check': () => Response.json({ username: 'ann' }),
		'/api/v1/auth/password-resets/redeem': () => new Response(null, { status: 204 })
	};
	sent = [];
	vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
		const path = new URL(String(input), location.href).pathname;
		sent.push({
			path,
			body: init?.body ? JSON.parse(String(init.body)) : undefined
		});
		return answers[path]();
	});
	location.hash = 'secret-code';
});

afterEach(() => {
	connectivity.setWorkOffline(false);
	vi.restoreAllMocks();
	navigation.goto.mockClear();
});

/** What the server answers for a link that is not live: a Problem with no detail. */
const notFound = () =>
	Response.json(
		{ status: 404, code: 'not_found', title: 'Not Found' },
		{ status: 404, headers: { 'content-type': 'application/problem+json' } }
	);

/** An answer that refuses with `status` and `code`, saying `detail`. */
const problem = (status: number, code: string, detail: string) => () =>
	Response.json({ status, code, title: 'Refused', detail }, { status });

/** Types the new password twice, `again` the second time, and presses the button. */
async function choose(password: string, again = password) {
	await page.getByLabelText('New password', { exact: true }).fill(password);
	await page.getByLabelText('New password again').fill(again);
	await page.getByRole('button', { name: 'Change password' }).click();
}

it('names the account, takes the code out of the address bar, and sends the new password to sign in with', async () => {
	history.replaceState({ kept: 1 }, '', location.href);
	render(Reset);
	await expect.element(page.getByText('ann', { exact: true })).toBeVisible();
	expect(location.hash).toBe('');
	// SvelteKit's router state stays with the page.
	expect(history.state).toEqual({ kept: 1 });
	expect(sent.find((r) => r.path.endsWith('/check'))?.body).toEqual({
		code: 'secret-code'
	});

	await choose('a long and good password');

	await vi.waitFor(() => expect(navigation.goto).toHaveBeenCalledWith('/login?reset=1'));
	expect(sent.find((r) => r.path.endsWith('/redeem'))?.body).toEqual({
		code: 'secret-code',
		password: 'a long and good password'
	});
});

it('sends nothing while the two passwords differ', async () => {
	render(Reset);
	await expect.element(page.getByText('ann', { exact: true })).toBeVisible();
	await choose('a long and good password', 'another long password');
	await expect.element(page.getByRole('alert')).toHaveTextContent('The passwords do not match.');
	expect(sent.some((r) => r.path.endsWith('/redeem'))).toBe(false);
	expect(navigation.goto).not.toHaveBeenCalled();
});

it('says a dead link is no longer valid, with no form', async () => {
	answers['/api/v1/auth/password-resets/check'] = notFound;
	render(Reset);
	await expect.element(page.getByText('This link is no longer valid.')).toBeVisible();
	await expect.element(page.getByLabelText('New password again')).not.toBeInTheDocument();
});

it('says the link is no longer valid when it dies before the password is sent', async () => {
	answers['/api/v1/auth/password-resets/redeem'] = notFound;
	render(Reset);
	await expect.element(page.getByText('ann', { exact: true })).toBeVisible();
	await choose('a long and good password');
	await expect.element(page.getByRole('alert')).toHaveTextContent('This link is no longer valid.');
	expect(navigation.goto).not.toHaveBeenCalled();
});

it('asks for the link again when the page has no code, without asking the server', async () => {
	location.hash = '';
	render(Reset);
	await expect
		.element(page.getByText('Open the reset link you were sent again to continue.'))
		.toBeVisible();
	await expect.element(page.getByText('no longer valid')).not.toBeInTheDocument();
	expect(sent.map((r) => r.path)).toEqual(['/api/v1/me']);
});

it('asks someone signed in to sign out first, and sends nothing but the check of who they are', async () => {
	answers['/api/v1/me'] = () => Response.json({ id: 'u1', username: 'bo', displayName: 'Bo' });
	render(Reset);
	await expect
		.element(page.getByText("You're signed in as Bo. Sign out first to use this link."))
		.toBeVisible();
	await expect.element(page.getByLabelText('New password again')).not.toBeInTheDocument();
	expect(location.hash).toBe('');
	expect(sent.map((r) => r.path)).toEqual(['/api/v1/me']);
});

it('shows the server’s 422 and keeps the form filled', async () => {
	const detail = 'This is a top-10 common password. Choose a longer or less predictable password.';
	answers['/api/v1/auth/password-resets/redeem'] = problem(422, 'weak_password', detail);
	render(Reset);
	await expect.element(page.getByText('ann', { exact: true })).toBeVisible();
	await choose('password');

	await expect.element(page.getByRole('alert')).toHaveTextContent(detail);
	await expect
		.element(page.getByLabelText('New password', { exact: true }))
		.toHaveValue('password');
	await expect.element(page.getByLabelText('New password again')).toHaveValue('password');
	expect(navigation.goto).not.toHaveBeenCalled();
});

it('turns Work offline off to check the link', async () => {
	connectivity.setWorkOffline(true);
	render(Reset);
	await expect.element(page.getByText('ann', { exact: true })).toBeVisible();
	expect(connectivity.workOffline).toBe(false);
});
