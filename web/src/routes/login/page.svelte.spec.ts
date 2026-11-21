import { page } from 'vitest/browser';
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const at = vi.hoisted(() => ({ url: new URL('https://jewelcase.test/login') }));
vi.mock('$app/state', () => ({ page: at }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
const ending = vi.hoisted(() => ({ now: false }));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<typeof import('$lib/api/client')>()),
	sessionEnding: () => ending.now
}));

const { default: Login } = await import('./+page.svelte');
const { connectivity } = await import('$lib/connectivity.svelte');

afterEach(() => {
	at.url = new URL('https://jewelcase.test/login');
	sessionStorage.clear();
	localStorage.clear();
	ending.now = false;
	connectivity.setWorkOffline(false);
	vi.restoreAllMocks();
});

it('says once that a session the server ended discarded unsent plays and changes', async () => {
	sessionStorage.setItem('jewelcase-discarded', '3');
	const first = render(Login);
	await expect
		.element(
			page.getByText(
				'Your session ended, so 3 unsent plays and changes from this device were discarded.'
			)
		)
		.toBeVisible();
	first.unmount();
	// Taken down as it was read, so the next login never meets it.
	expect(sessionStorage.getItem('jewelcase-discarded')).toBeNull();
	render(Login);
	await expect.element(page.getByText('Your session ended')).not.toBeInTheDocument();
});

it('shows no form, and leaves the notice for the page after the reload a session that ended is about to make', async () => {
	sessionStorage.setItem('jewelcase-discarded', '3');
	ending.now = true;
	render(Login);
	await expect.element(page.getByText('Signing out…')).toBeVisible();
	await expect.element(page.getByText('Your session ended')).not.toBeInTheDocument();
	await expect.element(page.getByLabelText('Username')).not.toBeInTheDocument();
	expect(sessionStorage.getItem('jewelcase-discarded')).toBe('3');
});

it('turns Work offline off to log in, as no page a signed-out user reaches has the switch', async () => {
	connectivity.setWorkOffline(true);
	const sent = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: 's1' }));
	render(Login);
	await page.getByLabelText('Username').fill('owner');
	await page.getByLabelText('Password').fill('secret');
	await page.getByRole('button', { name: 'Log in' }).click();
	await vi.waitFor(() => expect(sent).toHaveBeenCalled());
	expect(connectivity.workOffline).toBe(false);
});

/** Logs in as `owner` and answers with the body the server's login request carried. */
async function logInAs(answer: Response = Response.json({ id: 's1' })) {
	const sent = vi.spyOn(globalThis, 'fetch').mockResolvedValue(answer);
	render(Login);
	await page.getByLabelText('Username').fill('owner');
	await page.getByLabelText('Password').fill('secret');
	await page.getByRole('button', { name: 'Log in' }).click();
	await vi.waitFor(() => expect(sent).toHaveBeenCalled());
	return JSON.parse(String(sent.mock.calls[0][1]?.body));
}

it('sends the device ID a credentials change signed out, and lets it go once logged in', async () => {
	localStorage.setItem('jewelcase-reclaim', '7');
	expect(await logInAs()).toMatchObject({ username: 'owner', deviceId: '7' });
	await vi.waitFor(() => expect(localStorage.getItem('jewelcase-reclaim')).toBeNull());
});

it('keeps the device ID through a refused login, for the attempt with the new password', async () => {
	localStorage.setItem('jewelcase-reclaim', '7');
	const refusal = Response.json(
		{ code: 'invalid_credentials', title: 'Unauthorized', status: 401 },
		{ status: 401 }
	);
	expect(await logInAs(refusal)).toMatchObject({ deviceId: '7' });
	await expect.element(page.getByRole('alert')).toBeVisible();
	expect(localStorage.getItem('jewelcase-reclaim')).toBe('7');
});

it('sends no device ID when none was kept', async () => {
	expect(await logInAs()).not.toHaveProperty('deviceId');
});

it('says a reset password was changed, for the person sent here after choosing one', async () => {
	at.url = new URL('https://jewelcase.test/login?reset=1');
	render(Login);
	await expect
		.element(page.getByText('Your password was changed. Log in with the new one.'))
		.toBeVisible();
});

it('says nothing of a password change on an ordinary visit', async () => {
	render(Login);
	await expect.element(page.getByLabelText('Username')).toBeVisible();
	await expect.element(page.getByText('Your password was changed')).not.toBeInTheDocument();
});
