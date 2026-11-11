import { afterEach, expect, it, vi } from 'vitest';

vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));
vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: { onDevice: () => false, waitForDevice: async () => false, changes: 0 }
}));

const { sender } = await import('$lib/sync/sender');
const { memoryEnv } = await import('$lib/sync/store');
const { load } = await import('./+page');

afterEach(() => sender.stop());

it('waits for the server to have a playlist made here before reading it from the server', async () => {
	const seen: string[] = [];
	let answer!: () => void;
	const playlist = { id: '77', libraryId: '11', title: 'New', kind: 'saved', version: 1 };
	const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
		const url = new URL(String(input), 'http://test');
		seen.push(`${init.method ?? 'GET'} ${url.pathname.replace(/^.*\/api\/v1/, '')}`);
		if (init.method === 'POST') {
			await new Promise<void>((resolve) => (answer = resolve));
			return Response.json(playlist, { status: 201 });
		}
		return Response.json(
			url.pathname.endsWith('/items')
				? { items: [], nextCursor: null, total: 0, version: 1 }
				: playlist
		);
	}) as typeof globalThis.fetch;
	sender.start('1', { env: memoryEnv(), fetch, device: undefined });
	sender.enqueue('createPlaylist', { libraryId: '11', id: '77', title: 'New' });

	const loading = load({
		fetch,
		parent: async () => ({ library: { id: '11' } }),
		params: { id: '77' }
	} as unknown as Parameters<typeof load>[0]);
	await vi.waitFor(() => expect(seen).toEqual(['POST /libraries/11/playlists']));
	await new Promise((resolve) => setTimeout(resolve, 20));
	expect(seen).toEqual(['POST /libraries/11/playlists']);
	answer();
	const data = await loading;
	expect(data).toMatchObject({ lib: '11', loaded: { playlist: { id: '77' } } });
	expect(seen.slice(1).every((line) => line.startsWith('GET '))).toBe(true);
});
