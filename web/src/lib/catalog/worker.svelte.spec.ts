import { afterEach, expect, it, vi } from 'vitest';
import type { FromWorker, ToWorker } from './protocol';

/** Holds `start` until the test lets it go, as a long apply or index build does. */
const work = vi.hoisted(() => ({
	finish: () => {},
	running: false,
	signedOut: false,
	state: undefined as unknown as (libraries: Record<string, unknown>) => void,
	heavy: undefined as unknown as (on: boolean) => void
}));
vi.mock('$lib/catalog-wasm/jewelcase_catalog_wasm.js', () => ({ default: async () => {} }));
vi.mock('./sync', () => ({
	describe: (error: Error) => ({ message: String(error), quota: false }),
	CatalogSync: class {
		constructor(options: {
			state: (libraries: Record<string, unknown>) => void;
			heavy: (on: boolean) => void;
		}) {
			work.state = options.state;
			work.heavy = options.heavy;
		}
		start = () =>
			new Promise<void>((resolve) => {
				work.running = true;
				work.finish = resolve;
			});
		busy = () => true;
		signOut = async () => {
			work.signedOut = true;
		};
	}
}));

afterEach(() => vi.restoreAllMocks());

it('answers whether a download is under way while a start is still running, and passes on what the sync says of its libraries', async () => {
	const said: FromWorker[] = [];
	vi.spyOn(window, 'postMessage').mockImplementation(((message: FromWorker) =>
		said.push(message)) as never);
	await import('./worker');
	const send = (message: ToWorker) =>
		(window.onmessage as unknown as (e: { data: ToWorker }) => void)({ data: message });

	send({ type: 'init', api: '/api', wasm: '', user: '1' });
	send({ type: 'start' });
	await vi.waitFor(() => expect(work.running).toBe(true));
	send({ type: 'busy', id: 2 });
	await vi.waitFor(() => expect(said).toEqual([{ type: 'answer', id: 2, result: '1' }]));
	const libraries = { '1': { onDevice: true, applying: false } };
	work.state(libraries);
	expect(said.at(-1)).toEqual({ type: 'state', libraries });
	work.heavy(true);
	expect(said.at(-1)).toEqual({ type: 'heavy', on: true });
	work.heavy(false);
	expect(said.at(-1)).toEqual({ type: 'heavy', on: false });
	work.finish();
});

it('answers a request it does not know, as from a newer build, with an error, and keeps the files', async () => {
	const said: FromWorker[] = [];
	vi.spyOn(window, 'postMessage').mockImplementation(((message: FromWorker) =>
		said.push(message)) as never);
	await import('./worker');
	const send = (message: ToWorker) =>
		(window.onmessage as unknown as (e: { data: ToWorker }) => void)({ data: message });

	send({ type: 'init', api: '/api', wasm: '', user: '1' });
	send({ type: 'newer', id: 4 } as unknown as ToWorker);
	await vi.waitFor(() =>
		expect(said).toContainEqual({ type: 'answer', id: 4, error: expect.any(String) })
	);
	expect(work.signedOut).toBe(false);
});
