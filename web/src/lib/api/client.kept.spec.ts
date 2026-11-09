import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/catalog/client.svelte', () => ({ catalog: {} }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { offline: false } }));
vi.mock('$lib/connectivity.svelte', () => ({ connectivity: { workOffline: false } }));
vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

let getMe: typeof import('./client').getMe;
let getServerInfo: typeof import('./client').getServerInfo;

/** A browser storage the test can read and write. */
function storage(initial: Record<string, string>) {
	const items = new Map(Object.entries(initial));
	return {
		getItem: (key: string) => items.get(key) ?? null,
		setItem: (key: string, value: string) => void items.set(key, value),
		removeItem: (key: string) => void items.delete(key)
	};
}

beforeEach(async () => {
	vi.resetModules();
	({ getMe, getServerInfo } = await import('./client'));
	vi.useFakeTimers();
	vi.stubGlobal(
		'localStorage',
		storage({
			'jewelcase-last-known': JSON.stringify({ me: { id: '7' }, server: { setupRequired: false } })
		})
	);
});

afterEach(() => {
	vi.useRealTimers();
	vi.unstubAllGlobals();
});

describe('the layout’s kept reads', () => {
	it('fall back to what was kept when the server leaves the request hanging', async () => {
		const hang = vi.fn(() => new Promise<Response>(() => {}));
		const me = getMe(hang);
		await vi.advanceTimersByTimeAsync(3500);
		expect(await me).toEqual({ id: '7' });
	});

	it('wait for the server while nothing was kept, and use its answer when it comes in time', async () => {
		vi.stubGlobal('localStorage', storage({}));
		let answer: (response: Response) => void = () => {};
		const slow = vi.fn(() => new Promise<Response>((resolve) => (answer = resolve)));
		const me = getMe(slow);
		await vi.advanceTimersByTimeAsync(10_000);
		answer(Response.json({ id: '9' }));
		expect(await me).toEqual({ id: '9' });
	});

	it('wait once for a server that hangs, not once for each read of the start', async () => {
		const hang = vi.fn(() => new Promise<Response>(() => {}));
		const server = getServerInfo(hang);
		await vi.advanceTimersByTimeAsync(3500);
		expect(await server).toEqual({ setupRequired: false });

		// The next read of the start does not wait the three seconds again.
		let me: unknown;
		void getMe(hang).then((value) => (me = value));
		await vi.advanceTimersByTimeAsync(0);
		expect(me).toEqual({ id: '7' });
	});

	it('wait again once the server has answered after hanging', async () => {
		const hang = vi.fn(() => new Promise<Response>(() => {}));
		const first = getServerInfo(hang);
		await vi.advanceTimersByTimeAsync(3500);
		await first;

		// Hanging, so this read does not wait; the server's answer to it ends the hanging.
		await getMe(vi.fn(async () => Response.json({ id: '7' })));
		await vi.advanceTimersByTimeAsync(0);

		let done = false;
		void getServerInfo(hang).then(() => (done = true));
		await vi.advanceTimersByTimeAsync(2900);
		expect(done).toBe(false);
		await vi.advanceTimersByTimeAsync(200);
		expect(done).toBe(true);
	});
});
