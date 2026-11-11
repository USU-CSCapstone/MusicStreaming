import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { KeptQueue, RealtimeServerMessage } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	list: (async () => ({ items: [], nextCursor: null })) as (
		cursor?: string
	) => Promise<{ items: unknown[]; nextCursor: string | null }>,
	restore: (async () => null) as (id: string) => Promise<string | null>,
	/** What was given to the log, and who hears what became of it. */
	logged: [] as { name: string; args: { queueId: string } }[],
	settled: [] as ((rec: { name: string; args: unknown }, outcome: string) => void)[]
}));

vi.mock('$lib/sync/sender', () => ({
	sender: {
		enqueue: (name: string, args: { queueId: string }) => void fake.logged.push({ name, args }),
		onSettled: (listener: (rec: { name: string; args: unknown }, outcome: string) => void) => {
			fake.settled.push(listener);
			return () => fake.settled.splice(fake.settled.indexOf(listener), 1);
		}
	}
}));

vi.mock('$lib/realtime.svelte', () => ({
	realtime: {
		on: (listener: (message: RealtimeServerMessage) => void) => {
			fake.listeners.push(listener);
			return () => fake.listeners.splice(fake.listeners.indexOf(listener), 1);
		}
	}
}));
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
	listKeptQueues: (_: unknown, cursor?: string) => fake.list(cursor),
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

/** Delivers a realtime message to every listener the component registered. */
const hear = (message: object) =>
	fake.listeners.forEach((listener) => listener(message as RealtimeServerMessage));

beforeEach(() => {
	fake.listeners.length = 0;
	fake.list = async () => ({ items: [kept(1), kept(2)], nextCursor: null });
	fake.restore = async () => null;
	fake.logged = [];
	fake.settled.length = 0;
});

describe('KeptQueues', () => {
	it('shows what each queue was playing, where from, how much it holds, and when', async () => {
		fake.list = async () => ({
			items: [kept(1), kept(2, { current: undefined, context: { type: 'library' } })],
			nextCursor: null
		});
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

	it('refetches on queuesChanged, and when the connection returns', async () => {
		const asked = vi.fn(fake.list);
		fake.list = asked;
		render(KeptQueues);
		await expect.element(page.getByText('Song 2')).toBeInTheDocument();
		expect(asked).toHaveBeenCalledTimes(1);

		fake.list = async () => ({ items: [kept(3)], nextCursor: null });
		hear({ type: 'queuesChanged' });
		await expect.element(page.getByText('Song 3')).toBeInTheDocument();
		await expect.element(page.getByText('Song 2')).not.toBeInTheDocument();

		hear({ type: 'devices', devices: [] });
		fake.list = async () => ({ items: [kept(4)], nextCursor: null });
		hear({ type: 'hello', deviceId: '1', serverTime: '' });
		await expect.element(page.getByText('Song 4')).toBeInTheDocument();
	});

	it('pages with the cursor, and a refetch keeps the pages shown', async () => {
		const cursors: (string | undefined)[] = [];
		fake.list = async (cursor) => {
			cursors.push(cursor);
			return cursor
				? { items: [kept(3)], nextCursor: null }
				: { items: [kept(1), kept(2)], nextCursor: 'c1' };
		};
		render(KeptQueues);
		await expect.element(page.getByText('Song 2')).toBeInTheDocument();
		await expect.element(page.getByText('Song 3')).not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Show more' }).click();
		await expect.element(page.getByText('Song 3')).toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
		expect(cursors).toEqual([undefined, 'c1']);

		hear({ type: 'queuesChanged' });
		await expect.poll(() => cursors).toEqual([undefined, 'c1', undefined, 'c1']);
		await expect.element(page.getByText('Song 3')).toBeInTheDocument();
	});

	it('deletes with one press through the log, and shows again one the server refused', async () => {
		render(KeptQueues);
		await page.getByRole('button', { name: /Delete Song 1/ }).click();
		expect(fake.logged).toEqual([{ name: 'deleteKeptQueue', args: { queueId: 'q1' } }]);
		await expect.element(page.getByText('Song 1')).not.toBeInTheDocument();
		// A load before the server has it leaves it out.
		hear({ type: 'queuesChanged' });
		await new Promise((resolve) => setTimeout(resolve, 50));
		await expect.element(page.getByText('Song 1')).not.toBeInTheDocument();

		await page.getByRole('button', { name: /Delete Song 2/ }).click();
		await expect.element(page.getByText('Song 2')).not.toBeInTheDocument();
		// The log told the user why; the list is the server's again.
		for (const heard of fake.settled)
			heard({ name: 'deleteKeptQueue', args: { queueId: 'q2' } }, 'dropped');
		await expect.element(page.getByText('Song 2')).toBeInTheDocument();
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

	it('says kept queues need the server when it cannot be reached, and tries again', async () => {
		fake.list = async () => {
			throw new TypeError('offline');
		};
		render(KeptQueues);
		await expect
			.element(page.getByRole('status'))
			.toHaveTextContent('Kept queues need a connection to the server.');
		fake.list = async () => ({ items: [kept(1)], nextCursor: null });
		await page.getByRole('button', { name: 'Try again' }).click();
		await expect.element(page.getByText('Song 1')).toBeInTheDocument();
	});

	it('says what kept queues are when there are none', async () => {
		fake.list = async () => ({ items: [], nextCursor: null });
		render(KeptQueues);
		await expect.element(page.getByText(/kept here for 30 days/)).toBeInTheDocument();
	});
});
