import { expect, it, vi } from 'vitest';
import type { FromWorker, ToWorker } from './protocol';

/**
 * A read of library 1 waits, as a search waits for its index build or an offline read for an apply,
 * until the pause cuts it short.
 */
const work = vi.hoisted(() => ({ cut: (() => {}) as (error: Error) => void }));
vi.mock('$lib/catalog-wasm/jewelcase_catalog_wasm.js', () => ({ default: async () => {} }));
vi.mock('./sync', () => ({
	describe: (error: Error) => ({ message: String(error), quota: false }),
	CatalogSync: class {
		start = async () => [];
		query = (library: string) =>
			library === '1'
				? new Promise<string>((_, reject) => (work.cut = reject))
				: Promise.resolve(`answer${library}`);
		pause = async () => {
			// Closing frees the library, so the read waiting on it finds it gone.
			setTimeout(() => work.cut(new Error('the library is not on this device')));
		};
	}
}));

it('answers other libraries’ reads and a pause while one read waits, and leaves a read the pause cut short for the next owner', async () => {
	const said: FromWorker[] = [];
	vi.spyOn(window, 'postMessage').mockImplementation(((message: FromWorker) =>
		said.push(message)) as never);
	await import('./worker');
	const send = (message: ToWorker) =>
		(window.onmessage as unknown as (e: { data: ToWorker }) => void)({ data: message });
	const query = (id: number, library: string): ToWorker => ({
		type: 'query',
		id,
		library,
		name: 'search',
		params: {},
		offline: true
	});

	send({ type: 'init', api: '/api', wasm: '', user: '1' });
	send({ type: 'start' });
	send(query(1, '1'));
	send(query(2, '2'));
	send({ type: 'pause' });
	await vi.waitFor(() => expect(said).toContainEqual({ type: 'paused' }));
	expect(said).toContainEqual({ type: 'answer', id: 2, result: 'answer2' });
	await new Promise((resolve) => setTimeout(resolve, 50));
	// Unanswered, so the asking tab sends it to the owner that takes over.
	expect(said.filter((m) => m.type === 'answer' && m.id === 1)).toEqual([]);
});
