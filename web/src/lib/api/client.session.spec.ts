import { beforeEach, describe, expect, it, vi } from 'vitest';

const catalog = vi.hoisted(() => ({ signOut: vi.fn(async () => {}) }));
vi.mock('$lib/catalog/client.svelte', () => ({ catalog }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { offline: false } }));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

// The module ends a session once per page, as the page reloads after it: each test loads it anew.
let { listKeptQueues, login } = await import('./client');

/** A browser storage the test can read and write; the owner is `owner`, if any. The page is at
 * `/albums`, and `assign` records where it is sent. */
function signedInAs(owner?: string) {
	const items = new Map(owner ? [['jewelcase-owner', owner]] : []);
	vi.stubGlobal('localStorage', {
		getItem: (key: string) => items.get(key) ?? null,
		setItem: (key: string, value: string) => void items.set(key, value),
		removeItem: (key: string) => void items.delete(key)
	});
	vi.stubGlobal('location', { pathname: '/albums', search: '', assign });
}

const assign = vi.fn();

/** A fetch answering a Problem with `code`. */
const problem = (status: number, code: string) => async () =>
	Response.json({ code, title: code }, { status });

beforeEach(async () => {
	catalog.signOut.mockClear();
	assign.mockClear();
	vi.resetModules();
	({ listKeptQueues, login } = await import('./client'));
});

describe('a request the session no longer covers', () => {
	it.each(['unauthenticated', 'session_revoked'])(
		'wipes the device when a session that was good says %s',
		async (code) => {
			signedInAs('7');
			await expect(listKeptQueues(problem(401, code))).rejects.toMatchObject({ status: 307 });
			expect(catalog.signOut).toHaveBeenCalledOnce();
		}
	);

	it('reloads at the login page once the device is wiped, so nothing of the session stays in memory', async () => {
		signedInAs('7');
		let wiped!: () => void;
		catalog.signOut.mockImplementationOnce(() => new Promise<void>((done) => (wiped = done)));
		await expect(listKeptQueues(problem(401, 'unauthenticated'))).rejects.toMatchObject({
			status: 307,
			location: '/login?next=%2Falbums'
		});
		// A second request refused while the wipe runs ends nothing more.
		await expect(listKeptQueues(problem(401, 'unauthenticated'))).rejects.toMatchObject({
			status: 307
		});
		expect(assign).not.toHaveBeenCalled();
		wiped();
		await vi.waitFor(() => expect(assign).toHaveBeenCalledWith('/login?next=%2Falbums'));
		expect(catalog.signOut).toHaveBeenCalledTimes(1);
		expect(assign).toHaveBeenCalledTimes(1);
	});

	it('has nothing to wipe where no session was ever good', async () => {
		signedInAs();
		await expect(listKeptQueues(problem(401, 'unauthenticated'))).rejects.toMatchObject({
			status: 307
		});
		expect(catalog.signOut).not.toHaveBeenCalled();
		expect(assign).not.toHaveBeenCalled();
	});

	it('wipes nothing for a server that cannot be reached, or that fails', async () => {
		signedInAs('7');
		await expect(
			listKeptQueues(async () => Promise.reject(new TypeError('network')))
		).rejects.toThrow();
		await expect(listKeptQueues(problem(503, 'unavailable'))).rejects.toMatchObject({
			status: 503
		});
		await expect(listKeptQueues(problem(500, 'internal'))).rejects.toMatchObject({ status: 500 });
		expect(catalog.signOut).not.toHaveBeenCalled();
	});

	it('wipes nothing for a wrong password', async () => {
		signedInAs('7');
		await expect(
			login(problem(401, 'invalid_credentials'), { username: 'a', password: 'b' } as never)
		).rejects.toMatchObject({ code: 'invalid_credentials' });
		expect(catalog.signOut).not.toHaveBeenCalled();
	});
});
