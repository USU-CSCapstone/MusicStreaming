import { page } from 'vitest/browser';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { claim, remember } from '$lib/last-known';

const fake = vi.hoisted(() => ({
	calls: [] as string[],
	role: 'owner' as 'owner' | 'admin' | 'user',
	libraries: {} as Record<string, object>,
	signedOutOthers: 0,
	passwordChanges: [] as object[],
	passwordFailure: null as Error | null,
	failure: null as Error | null
}));

vi.mock('$lib/realtime.svelte', () => ({
	realtime: {
		connected: false,
		offline: false,
		deviceId: '2',
		on: () => () => {},
		start: () => fake.calls.push('start'),
		stop: () => fake.calls.push('stop')
	}
}));
vi.mock('$lib/api/client', async (original) => ({
	...(await original<object>()),
	changeMyPassword: async (_f: unknown, body: object) => {
		fake.passwordChanges.push(body);
		if (fake.passwordFailure) throw fake.passwordFailure;
	},
	signOutMyOtherDevices: async () => {
		fake.signedOutOthers++;
		if (fake.failure) throw fake.failure;
	}
}));
vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: {
		signOut: async () => void fake.calls.push('wipe'),
		get libraries() {
			return fake.libraries;
		}
	}
}));
vi.mock('$lib/last-known', async (original) => ({
	...(await original<object>()),
	setLogoutPending: (on: boolean) => fake.calls.push(`marker ${on}`)
}));
// The server is never heard from, so the page stays where it is for the test to look at.
vi.mock('$lib/logout', () => ({
	finishLogout: () => {
		fake.calls.push('told');
		return new Promise(() => {});
	}
}));
vi.mock('$app/state', () => ({
	page: {
		route: { id: '/account' },
		get data() {
			return { me: { displayName: 'Owner', username: 'owner', role: fake.role }, tagTypes: [] };
		}
	}
}));

const { default: Account } = await import('./+page.svelte');
const { connectivity } = await import('$lib/connectivity.svelte');
const { sender } = await import('$lib/sync/sender');
const { playback } = await import('$lib/playback.svelte');
const { ApiError } = await import('$lib/api/root');
const { memoryEnv } = await import('$lib/sync/store');

beforeEach(() => {
	fake.calls.length = 0;
	fake.role = 'owner';
	fake.libraries = {};
	fake.signedOutOthers = 0;
	fake.failure = null;
	fake.passwordChanges = [];
	fake.passwordFailure = null;
	playback.devices = [];
	localStorage.clear();
	// The wait for the server is never over, so the page does not go to the login page.
	vi.useFakeTimers({ toFake: ['setTimeout'] });
});
afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
	sender.stop();
	connectivity.setWorkOffline(false);
});

it('wipes the device before it tells the server, so a server out of reach cannot keep the session on it', async () => {
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	// Realtime stops first, so its sign-out cannot race the login redirect; the marker is down
	// before the wipe, in case the page closes part-way.
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('asks before deleting what has not reached the server, and deletes only on yes', async () => {
	connectivity.setWorkOffline(true);
	sender.start('1', { env: memoryEnv() });
	sender.enqueue('play', { playId: 'p1' } as never);
	sender.enqueue('mark', { library: '1', track: '5', liked: true, what: 'like' });
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect
		.element(page.getByText("2 plays and changes haven't reached the server. Log out anyway?"))
		.toBeVisible();
	expect(fake.calls).toEqual([]);

	// No is no: nothing is deleted, and the button is back.
	await page.getByRole('button', { name: 'Cancel' }).click();
	expect(fake.calls).toEqual([]);
	await expect.element(page.getByRole('button', { name: 'Log out', exact: true })).toBeVisible();
	expect(await sender.pending()).toBe(2);

	await page.getByRole('button', { name: 'Log out', exact: true }).click();
	await page.getByRole('button', { name: 'Log out anyway' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('counts what was set aside among what has not reached the server', async () => {
	connectivity.setWorkOffline(true);
	const env = memoryEnv();
	await env.open('1').put('dead', { key: 'd', rev: 'r', owner: 'tab', createdAt: 1, tries: 20 });
	sender.start('1', { env });
	sender.enqueue('play', { playId: 'p1' } as never);
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect
		.element(
			page.getByText(
				"2 plays and changes haven't reached the server, 1 of them set aside. Log out anyway?"
			)
		)
		.toBeVisible();
});

it('offers to retry or discard a change set aside, but only to discard a queue change', async () => {
	const env = memoryEnv();
	const port = env.open('1');
	const aside = { rev: 'r', owner: 'tab', createdAt: 1, tries: 20, setAsideAt: 2, why: '' };
	await port.put('dead', {
		...aside,
		key: 'm',
		name: 'mark',
		scope: 'mark:1/5',
		args: { library: '1', track: '5', liked: true, what: 'like “Signal”' }
	} as never);
	await port.put('dead', {
		...aside,
		key: 'q',
		name: 'startQueue',
		scope: 'queue',
		args: {}
	} as never);
	sender.start('1', { env });
	render(Account, { data: { connectable: [] } } as never);
	const entry = (text: string) => page.getByRole('listitem').filter({ hasText: text });
	await expect.element(entry('Like “Signal”').getByRole('button', { name: 'Retry' })).toBeVisible();
	await expect
		.element(entry('Like “Signal”').getByRole('button', { name: 'Discard' }))
		.toBeVisible();
	await expect
		.element(entry('Start that queue').getByRole('button', { name: 'Discard' }))
		.toBeVisible();
	await expect
		.element(entry('Start that queue').getByRole('button', { name: 'Retry' }))
		.not.toBeInTheDocument();
});

it('puts the search sharing switch back when the server refuses it, and keeps it so', async () => {
	const refuse = (async () =>
		new Response(JSON.stringify({ status: 404, code: 'not_found', detail: 'Gone.' }), {
			status: 404
		})) as typeof fetch;
	claim('1');
	sender.start('1', { env: memoryEnv(), fetch: refuse });
	const plugin = {
		id: 'scrobbler',
		name: 'Scrobbler',
		connected: true,
		asksForSearches: true,
		sharesSearches: false
	};
	render(Account, { data: { connectable: [plugin] } } as never);
	const share = page.getByRole('checkbox', { name: /Share my searches with Scrobbler/ });
	await share.click();
	await sender.idle();
	await expect.element(share).not.toBeChecked();
	const { lastKnown } = await import('$lib/last-known');
	expect(lastKnown().plugins?.[0].sharesSearches).toBe(false);
});

it('logs out without asking when everything has been sent', async () => {
	sender.start('1', { env: memoryEnv() });
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
});

it('tries to send what is waiting before it asks, and logs out without asking once it is sent', async () => {
	const posted: string[] = [];
	let answers = 0;
	/** A server that is busy for its first answer, then accepts the plays it is sent. */
	const fetcher = (async (_input: RequestInfo | URL, init: RequestInit = {}) => {
		// The first try is refused as the server being busy, so the play waits for its time.
		if (answers++ === 0) return new Response('{}', { status: 503 });
		for (const item of JSON.parse(String(init.body)).items) posted.push(item.playId);
		return new Response(null, { status: 204 });
	}) as typeof fetch;
	sender.start('1', { env: memoryEnv(), fetch: fetcher });
	sender.enqueue('play', { playId: 'p1' } as never);
	await vi.waitFor(() => expect(answers).toBe(1));
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Log out' }).click();
	await vi.waitFor(() => expect(fake.calls).toEqual(['stop', 'marker true', 'wipe', 'told']));
	expect(posted).toEqual(['p1']);
	await expect.element(page.getByText("haven't reached the server")).not.toBeInTheDocument();
});

it.each([
	[true, 'The browser has agreed to keep it.'],
	[false, 'The browser has not agreed to keep it, and may clear it.']
])('says whether the browser granted persistent storage (%s)', async (granted, text) => {
	vi.spyOn(navigator.storage, 'persisted').mockResolvedValue(granted);
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByText(text)).toBeVisible();
});

it('offers administration to an admin, and not to a user', async () => {
	fake.role = 'admin';
	const admin = page.getByRole('region', { name: 'Administration' });
	const shown = render(Account, { data: { connectable: [] } } as never);
	await expect.element(admin.getByRole('link', { name: 'Libraries' })).toBeVisible();
	await expect.element(admin.getByRole('link', { name: 'Plugins' })).toBeVisible();
	// The owner alone reads the audit log.
	expect(admin.getByRole('link', { name: 'Audit log' }).elements()).toHaveLength(0);
	expect(
		admin.getByText('Find an account and manage its access, status, password and role.').elements()
	).toHaveLength(1);
	shown.unmount();

	fake.role = 'owner';
	const owner = render(Account, { data: { connectable: [] } } as never);
	await expect.element(admin.getByRole('link', { name: 'Audit log' })).toBeVisible();
	owner.unmount();

	fake.role = 'user';
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByRole('heading', { name: 'Devices' })).toBeVisible();
	expect(admin.elements()).toHaveLength(0);
});

it('says plainly, per library, what the device holds and which download it left for want of room', async () => {
	claim('1');
	remember({
		libraries: {
			items: [
				{ id: '1', name: 'Main' },
				{ id: '2', name: 'Vinyl' }
			] as never
		}
	});
	fake.libraries = {
		'1': { onDevice: true, applying: false, indexed: false },
		'2': {
			onDevice: false,
			applying: false,
			indexed: false,
			noRoom: { need: 540_000_000, free: 120_000_000 }
		}
	};
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByText('On this device', { exact: true })).toBeVisible();
	await expect.element(page.getByText('Main')).toBeVisible();
	await expect.element(page.getByText('Vinyl')).toBeVisible();
	await expect
		.element(page.getByText('Not downloaded: a copy needs 540 MB free and this device has 120 MB.'))
		.toBeVisible();
});

it('says of a library already on the device that its new copy was not downloaded, and that it keeps up from the changes', async () => {
	fake.libraries = {
		'1': {
			onDevice: true,
			applying: false,
			indexed: false,
			noRoom: { need: 540_000_000, free: 120_000_000 }
		}
	};
	render(Account, { data: { connectable: [] } } as never);
	await expect
		.element(
			page.getByText(
				'Not downloaded again: a new copy needs 540 MB free and this device has 120 MB.'
			)
		)
		.toBeVisible();
	await expect.element(page.getByText('It keeps up with the server’s changes')).toBeVisible();
});

const device = (id: string, name: string) =>
	({ id, name, type: 'desktop', connected: true, firstSeenAt: '', lastSeenAt: '' }) as never;

it('offers to sign out all other devices only when another is signed in', async () => {
	playback.devices = [device('2', 'Phone')];
	render(Account, { data: { connectable: [] } } as never);
	await expect.element(page.getByRole('list', { name: 'Devices' })).toBeVisible();
	await expect
		.element(page.getByRole('button', { name: 'Sign out all other devices' }))
		.not.toBeInTheDocument();
	playback.devices = [device('2', 'Phone'), device('1', 'Desk')];
	await expect
		.element(page.getByRole('button', { name: 'Sign out all other devices' }))
		.toBeVisible();
});

it('asks before signing out all other devices, and signs them out only on yes', async () => {
	playback.devices = [device('2', 'Phone'), device('1', 'Desk')];
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Sign out all other devices' }).click();
	await expect.element(page.getByRole('dialog')).toBeVisible();
	expect(fake.signedOutOthers).toBe(0);

	await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).click();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	expect(fake.signedOutOthers).toBe(0);

	await page.getByRole('button', { name: 'Sign out all other devices' }).click();
	await page
		.getByRole('dialog')
		.getByRole('button', { name: 'Sign out all other devices' })
		.click();
	await vi.waitFor(() => expect(fake.signedOutOthers).toBe(1));
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
});

it("shows the server's reason in the dialog when signing out the others fails", async () => {
	playback.devices = [device('2', 'Phone'), device('1', 'Desk')];
	fake.failure = new ApiError(500, undefined, 'Internal Server Error');
	render(Account, { data: { connectable: [] } } as never);
	await page.getByRole('button', { name: 'Sign out all other devices' }).click();
	await page
		.getByRole('dialog')
		.getByRole('button', { name: 'Sign out all other devices' })
		.click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/^Internal Server Error$/);
});

/** Fills the change-password form: the current password, the new one, and the new one again. */
async function fillPassword(current: string, next: string, again: string) {
	await page.getByLabelText('Current password').fill(current);
	await page.getByLabelText('New password', { exact: true }).fill(next);
	await page.getByLabelText('New password again').fill(again);
	await page.getByRole('button', { name: 'Change password' }).click();
}

it('changes the password, clears the form and says the other devices were signed out', async () => {
	render(Account, { data: { connectable: [] } } as never);
	await expect
		.element(page.getByLabelText('Current password'))
		.toHaveAttribute('autocomplete', 'current-password');
	await expect
		.element(page.getByLabelText('New password', { exact: true }))
		.toHaveAttribute('autocomplete', 'new-password');
	await fillPassword('old one', 'a brand new passphrase', 'a brand new passphrase');
	await expect
		.element(page.getByRole('status'))
		.toHaveTextContent('Your password was changed and your other devices were signed out.');
	expect(fake.passwordChanges).toEqual([
		{ currentPassword: 'old one', newPassword: 'a brand new passphrase' }
	]);
	await expect.element(page.getByLabelText('Current password')).toHaveValue('');
	await expect.element(page.getByLabelText('New password', { exact: true })).toHaveValue('');
	await expect.element(page.getByLabelText('New password again')).toHaveValue('');
});

it.each([
	[403, 'wrong_password', 'Your current password is not correct.'],
	[429, 'rate_limited', 'Too many wrong passwords. Wait a moment and try again.']
])(
	'shows the server’s %i %s message, and keeps the form and the session',
	async (status, code, detail) => {
		fake.passwordFailure = new ApiError(status, code, detail);
		render(Account, { data: { connectable: [] } } as never);
		await fillPassword('wrong', 'a brand new passphrase', 'a brand new passphrase');
		await expect.element(page.getByRole('alert')).toHaveTextContent(detail);
		await expect.element(page.getByLabelText('Current password')).toHaveValue('wrong');
		await expect.element(page.getByRole('status')).not.toBeInTheDocument();
		expect(fake.calls).not.toContain('wipe');
	}
);

it('shows a weak password’s message as the server sends it', async () => {
	const detail = 'This is a top-10 common password. Choose a longer or less predictable password.';
	fake.passwordFailure = new ApiError(422, 'weak_password', detail);
	render(Account, { data: { connectable: [] } } as never);
	await fillPassword('old one', 'password', 'password');
	await expect.element(page.getByRole('alert')).toHaveTextContent(detail);
});

it('refuses new passwords that differ, without sending', async () => {
	render(Account, { data: { connectable: [] } } as never);
	await fillPassword('old one', 'a brand new passphrase', 'a different one');
	await expect
		.element(page.getByRole('alert'))
		.toHaveTextContent('The new passwords do not match.');
	expect(fake.passwordChanges).toEqual([]);
});
