import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Tag } from '$lib/api/types';
import { type TagFilter, UNFILTERED } from '$lib/filters';

const tag = (id: string, type: string, name: string, trackCount: number): Tag => ({
	id,
	type,
	name,
	trackCount,
	albumCount: 1,
	artistCount: 1
});

// The library's genres and its one mood, most used first as the server lists them.
const TAGS = [
	tag('30', 'genre', 'Rock', 120),
	tag('33', 'genre', 'Électro', 40),
	tag('31', 'genre', 'Hip-Hop', 12),
	tag('32', 'mood', 'Calm', 7)
];

const api = vi.hoisted(() => ({ listed: [] as string[] }));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	listTagTypes: async () => [
		{ key: 'genre', tagCount: 3 },
		{ key: 'mood', tagCount: 1 }
	],
	listTags: async (_f: unknown, _library: string, query: { type: string }) => {
		api.listed.push(query.type);
		return { items: TAGS.filter((t) => t.type === query.type), nextCursor: null, total: 0 };
	},
	getTag: async (_f: unknown, _library: string, id: string) => {
		const found = TAGS.find((t) => t.id === id);
		if (!found) throw new Error('not found');
		return found;
	}
}));

const { default: FilterBar } = await import('./FilterBar.svelte');

/** The bar showing `filter`, and every filter it hands back. */
function show(filter: TagFilter = UNFILTERED) {
	const changes: TagFilter[] = [];
	render(FilterBar, { libraryId: '1', filter, onchange: (f: TagFilter) => changes.push(f) });
	return changes;
}

const button = (name: string | RegExp) => page.getByRole('button', { name });

describe('FilterBar.svelte', () => {
	it('has a button for each type, and nothing else until a tag is chosen', async () => {
		show();
		await expect.element(button('Genre')).toBeVisible();
		await expect.element(button('Mood')).toBeVisible();
		await expect.element(button('Clear filters')).not.toBeInTheDocument();
		expect(api.listed).toEqual([]);
	});

	it("lists a type's tags most used first, and finds them ignoring case and accents", async () => {
		const changes = show();
		await button('Genre').click();
		const genres = page.getByRole('list', { name: 'Genres' }).getByRole('checkbox');
		await expect.element(genres.nth(0)).toHaveAccessibleName(/Rock/);
		await expect.element(genres.nth(2)).toHaveAccessibleName(/Hip-Hop/);
		await userEvent.fill(page.getByRole('searchbox', { name: 'Find a genre' }), 'ELECT');
		await expect.element(genres).toHaveAccessibleName(/Électro/);
		await genres.click();
		expect(changes).toEqual([{ tags: ['33'], match: 'any' }]);
		await userEvent.fill(page.getByRole('searchbox', { name: 'Find a genre' }), 'jazz');
		await expect.element(page.getByText('No genre matches “jazz”.')).toBeVisible();
	});

	it('shows the chosen tags by name, from the URL, and takes one out', async () => {
		const changes = show({ tags: ['30', '32', '99'], match: 'any' });
		await expect.element(button('Genre, 1 chosen')).toBeVisible();
		await expect.element(button('Mood, 1 chosen')).toBeVisible();
		await expect.element(page.getByText('Unknown tag')).toBeVisible();
		await button('Remove genre Rock').click();
		await button('Remove tag Unknown tag').click();
		expect(changes).toEqual([
			{ tags: ['32', '99'], match: 'any' },
			{ tags: ['30', '32'], match: 'any' }
		]);
	});

	it('offers nothing to match with one tag of each type', async () => {
		show({ tags: ['30', '32'], match: 'any' });
		await expect.element(button('Remove genre Rock')).toBeVisible();
		await expect.element(button('Remove mood Calm')).toBeVisible();
		await expect.element(page.getByRole('radiogroup')).not.toBeInTheDocument();
	});

	it('offers all or any once one type has two tags chosen', async () => {
		const changes = show({ tags: ['30', '31'], match: 'any' });
		await expect.element(page.getByRole('radio', { name: 'Any' })).toBeChecked();
		await page.getByRole('radio', { name: 'All' }).click();
		await button('Clear filters').click();
		expect(changes).toEqual([
			{ tags: ['30', '31'], match: 'all' },
			{ tags: [], match: 'any' }
		]);
	});

	it('closes on Escape, handing focus back to its button', async () => {
		show();
		await button('Mood').click();
		await expect.element(page.getByRole('dialog', { name: 'Mood filter' })).toBeVisible();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
		await expect.element(button('Mood')).toHaveFocus();
	});
});
