import { afterEach, expect, it, vi } from 'vitest';
import type { FromWorker, ToWorker } from './protocol';

/** Holds `start` until the test lets it go, as a long apply or index build does. */
const work = vi.hoisted(() => ({ finish: () => {}, running: false }));
vi.mock('$lib/catalog-wasm/jewelcase_catalog_wasm.js', () => ({ default: async () => {} }));
vi.mock('./sync', () => ({
	describe: (error: Error) => ({ message: String(error), quota: false }),
	CatalogSync: class {
		start = () =>
			new Promise<void>((resolve) => {
				work.running = true;
				work.finish = resolve;
			});
		onDevice = () => true;
		busy = () => true;
	}
}));

afterEach(() => vi.restoreAllMocks());

it('answers whether a library is on the device, and whether a download is under way, while a start is still running', async () => {
	const said: FromWorker[] = [];
	vi.spyOn(window, 'postMessage').mockImplementation(((message: FromWorker) =>
		said.push(message)) as never);
	await import('./worker');
	const send = (message: ToWorker) =>
		(window.onmessage as unknown as (e: { data: ToWorker }) => void)({ data: message });

	send({ type: 'init', api: '/api', wasm: '', user: '1' });
	send({ type: 'start' });
	await vi.waitFor(() => expect(work.running).toBe(true));
	send({ type: 'onDevice', id: 1, library: '1' });
	send({ type: 'busy', id: 2 });
	await vi.waitFor(() =>
		expect(said).toEqual([
			{ type: 'answer', id: 1, result: '1' },
			{ type: 'answer', id: 2, result: '1' }
		])
	);
	work.finish();
});
