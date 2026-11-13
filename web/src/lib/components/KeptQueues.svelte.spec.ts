import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { KeptQueue } from '$lib/api/types';
import type { FromWorker, ToWorker } from '$lib/queue/protocol';

const fake = vi.hoisted(() => ({
	restore: (async () => null) as (id: string) => Promise<string | null>,
	/** What the page told the queue worker, and how the test speaks for it. */
	worker: {
		sent: [] as unknown[],
		onmessage: null as ((event: MessageEvent) => void) | null,
		postMessage(message: unknown) {
			this.sent.push(message);
		},
		terminate() {},
		say(message: unknown) {
			this.onmessage?.({ data: message } as MessageEvent);
		}
	}
}));

vi.mock('$lib/queue/client.svelte', async (actual) => {
	const module = await actual<typeof import('$lib/queue/client.svelte')>();
	const queue = new module.QueueClient(() => fake.worker as unknown as Worker, {
		enqueue: () => {},
		onSettled: () => () => {}
	});
	queue.connect({ on: () => () => {} }, '7');
	return { ...module, queue };
});
vi.mock('$lib/player.svelte', () => ({ player: { restore: (id: string) => fake.restore(id) } }));
vi.mock('$lib/queue/summaries', () => ({
	trackKey: (libraryId: string, trackId: string) => `${libraryId}/${trackId}`,
	lookup: async (refs: { libraryId: string; trackId: string }[]) =>
		new Map(
			refs.map((r) => [
				`${r.libraryId}/${r.trackId}`,
				{ id: r.trackId, title: `Song ${r.trackId}` }
			])
		)
}));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getAlbum: async () => ({ title: 'Signal' }),
	getArtist: async () => ({ name: 'Nobody' }),
	getPlaylist: async () => ({ title: 'Mix' })
}));

const { default: KeptQueues } = await import('./KeptQueues.svelte');

/** A kept queue that was playing track `n` of an album. */
const kept = (n: number, changes: Partial<KeptQueue> = {}): KeptQueue => ({
	queueId: `q${n}`,
	context: { type: 'album', id: '9' },
	lastActive: new Date(Date.now() - 3 * 86_400_000).toISOString(),
	current: { libraryId: '11', trackId: String(n) },
	upNext: 4,
	history: 2,
	...changes
});

/** The worker says the kept queues are `entries`. */
const hold = (entries: KeptQueue[]) =>
	fake.worker.say({ type: 'kept', entries } satisfies FromWorker);

beforeEach(() => {
	fake.restore = async () => null;
	fake.worker.sent = [];
	hold([kept(1), kept(2)]);
});

describe('KeptQueues', () => {
	it('shows what each queue was playing, where from, how much it holds, and when', async () => {
		hold([kept(1), kept(2, { current: undefined, context: { type: 'library' } })]);
		render(KeptQueues);
		const list = page.getByRole('list', { name: 'Kept queues' });
		const first = list.getByRole('listitem').first();
		await expect.element(first).toHaveTextContent('Song 1');
		await expect.element(first).toHaveTextContent('Signal');
		await expect.element(first).toHaveTextContent('4 songs up next');
		await expect.element(first).toHaveTextContent('2 plays played');
		await expect.element(first).toHaveTextContent('3 days ago');
		const second = list.getByRole('listitem').nth(1);
		await expect.element(second).toHaveTextContent('Nothing was playing');
		await expect.element(second).toHaveTextContent('Your library');
	});

	it("shows the device's list as it changes, an entry that stays kept in place", async () => {
		render(KeptQueues);
		await expect.element(page.getByText('Song 2')).toBeInTheDocument();
		const second = page.getByRole('listitem').nth(1).element();
		hold([kept(3), kept(1), kept(2)]);
		await expect.element(page.getByText('Song 3')).toBeInTheDocument();
		expect(page.getByRole('listitem').nth(2).element()).toBe(second);
		hold([kept(3)]);
		await expect.element(page.getByText('Song 2')).not.toBeInTheDocument();
	});

	it('draws the list a part at a time', async () => {
		hold(Array.from({ length: 60 }, (_, i) => kept(i + 1)));
		render(KeptQueues);
		await expect
			.element(page.getByRole('button', { name: /^Restore Song 50\b/ }))
			.toBeInTheDocument();
		await expect
			.element(page.getByRole('button', { name: /^Restore Song 51\b/ }))
			.not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Show more' }).click();
		await expect
			.element(page.getByRole('button', { name: /^Restore Song 60\b/ }))
			.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
	});

	it('deletes with one press through the queue worker', async () => {
		render(KeptQueues);
		await page.getByRole('button', { name: /Delete Song 1/ }).click();
		expect(fake.worker.sent).toContainEqual({ type: 'deleteKept', queueId: 'q1' } as ToWorker);
	});

	it('restores the queue pressed, and says why one was not', async () => {
		const restored = vi.fn(async (id: string) => (id === 'q2' ? 'That queue is gone.' : null));
		fake.restore = restored;
		const onrestored = vi.fn();
		render(KeptQueues, { onrestored });
		await page.getByRole('button', { name: /Restore Song 1/ }).click();
		expect(restored).toHaveBeenCalledWith('q1');
		await expect.poll(() => onrestored).toHaveBeenCalledTimes(1);

		await page.getByRole('button', { name: /Restore Song 2/ }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('That queue is gone.');
		expect(onrestored).toHaveBeenCalledTimes(1);
	});

	it('says what kept queues are when there are none', async () => {
		hold([]);
		render(KeptQueues);
		await expect.element(page.getByText(/kept here for 30 days/)).toBeInTheDocument();
	});
});
