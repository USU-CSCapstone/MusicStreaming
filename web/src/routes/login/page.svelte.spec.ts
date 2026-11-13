import { page } from 'vitest/browser';
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

vi.mock('$app/state', () => ({ page: { url: new URL('https://jewelcase.test/login') } }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
const ending = vi.hoisted(() => ({ now: false }));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<typeof import('$lib/api/client')>()),
	sessionEnding: () => ending.now
}));

const { default: Login } = await import('./+page.svelte');
const { connectivity } = await import('$lib/connectivity.svelte');

afterEach(() => {
	sessionStorage.clear();
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
