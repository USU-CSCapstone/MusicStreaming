import { expect, it } from 'vitest';
import { AdminList } from './admin-list.svelte';

const pages = (cursor?: string) =>
	Promise.resolve(
		cursor === 'c1' ? { items: [3, 4], nextCursor: null } : { items: [1, 2], nextCursor: 'c1' }
	);

it('pages in, starts over, and replaces a row', async () => {
	const list = new AdminList(await pages(), pages);
	expect(list.hasMore).toBe(true);
	await list.loadMore();
	expect(list.items).toEqual([1, 2, 3, 4]);
	expect(list.hasMore).toBe(false);

	list.replace(9, (n) => n === 3);
	expect(list.items).toEqual([1, 2, 9, 4]);

	await list.reload();
	expect(list.items).toEqual([1, 2]);
	expect(list.hasMore).toBe(true);
});
