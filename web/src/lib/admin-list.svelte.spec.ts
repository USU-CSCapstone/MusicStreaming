import { expect, it } from 'vitest';
import { AdminList } from './admin-list.svelte';

/** Rows with the IDs given, in order. */
const rows = (...ids: string[]) => ids.map((id) => ({ id }));
/** Two pages of rows: 1 and 2 first, then 3 and 4 after the cursor `c1`. */
const pages = (cursor?: string) =>
	Promise.resolve(
		cursor === 'c1'
			? { items: rows('3', '4'), nextCursor: null }
			: { items: rows('1', '2'), nextCursor: 'c1' }
	);

it('pages in, starts over, and replaces a row', async () => {
	const list = new AdminList(await pages(), pages);
	expect(list.hasMore).toBe(true);
	await list.loadMore();
	expect(list.items).toEqual(rows('1', '2', '3', '4'));
	expect(list.hasMore).toBe(false);

	const newer = { id: '3', name: 'newer' };
	list.replace(newer);
	expect(list.items).toEqual([...rows('1', '2'), newer, ...rows('4')]);

	await list.reload();
	expect(list.items).toEqual(rows('1', '2'));
	expect(list.hasMore).toBe(true);
});

it('removes the row it is told to, keeping every loaded page and the cursor', async () => {
	const list = new AdminList(await pages(), pages);
	await list.loadMore();
	list.remove('3');
	expect(list.items).toEqual(rows('1', '2', '4'));
	expect(list.hasMore).toBe(false);

	const first = new AdminList(await pages(), pages);
	first.remove('1');
	expect(first.items).toEqual(rows('2'));
	expect(first.hasMore).toBe(true);
	await first.loadMore();
	expect(first.items).toEqual(rows('2', '3', '4'));
});
