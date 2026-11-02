import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Tag } from '$lib/api/types';
import type { TagMatch } from '$lib/filters';

const tag = (id: string, name: string, trackCount: number): Tag => ({
	id,
	type: 'genre',
	name,
	trackCount,
	albumCount: 1,
	artistCount: 1
});

// The library's genres, most used first as the server lists them.
const GENRES = [tag('30', 'Rock', 120), tag('33', 'Électro', 40), tag('31', 'Hip-Hop', 12)];

// Moods, of which there are more than a page holds: the most used one is read, and the server
// finds the rest. Each mood read is noted, with the search it was for.
const asked = vi.hoisted(() => [] as (string | undefined)[]);
const CALM = tag('41', 'Calm', 9);

vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	listTags: async (_f: unknown, _l: string, query: { type: string; q?: string }) => {
		if (query.type !== 'mood') return { items: GENRES, nextCursor: null, total: GENRES.length };
		asked.push(query.q);
		return query.q
			? { items: query.q === 'calm' ? [CALM] : [], nextCursor: null, total: 1 }
			: { items: [tag('40', 'Dark', 300)], nextCursor: 'more', total: 1500 };
	}
}));

const { default: TagPicker } = await import('./TagPicker.svelte');

/**
 * The genre picker, or with `key` another type's, as a column's heading, with `chosen` chosen and
 * matched by `match`, `null` for a mix; and what it hands back.
 */
function show(chosen: string[] = [], match: TagMatch | null = 'any', key = 'genre') {
	const did: unknown[] = [];
	render(TagPicker, {
		libraryId: '1',
		type: { key, tagCount: 3 },
		label: key === 'genre' ? 'Genre' : 'Mood',
		chosen,
		variant: 'heading',
		match: match ?? undefined,
		ontoggle: (t: Tag) => did.push(t.id),
		onmatch: (m: TagMatch) => did.push(m),
		onclear: () => did.push('clear')
	});
	return did;
}

const heading = page.getByRole('button', { name: /^Filter by genre/ });

describe('TagPicker.svelte', () => {
	it("lists a type's tags most used first, and finds them ignoring case and accents", async () => {
		const did = show();
		await heading.click();
		const genres = page.getByRole('list', { name: 'Genres' }).getByRole('checkbox');
		await expect.element(genres.nth(0)).toHaveAccessibleName(/Rock/);
		await expect.element(genres.nth(2)).toHaveAccessibleName(/Hip-Hop/);
		await userEvent.fill(page.getByRole('searchbox', { name: 'Find a genre' }), 'ELECT');
		await expect.element(genres).toHaveAccessibleName(/Électro/);
		await genres.click();
		expect(did).toEqual(['33']);
		await userEvent.fill(page.getByRole('searchbox', { name: 'Find a genre' }), 'jazz');
		await expect.element(page.getByText('No genre matches “jazz”.')).toBeVisible();
	});

	it('says how many are chosen, and offers all or any once two are', async () => {
		show(['30']);
		await expect
			.element(page.getByRole('button', { name: 'Filter by genre, 1 chosen' }))
			.toBeVisible();
		await page.getByRole('button', { name: /^Filter by genre/ }).click();
		await expect.element(page.getByRole('radiogroup')).not.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Clear' })).toBeVisible();
	});

	it('switches to all, and clears its own tags', async () => {
		const did = show(['30', '31']);
		await heading.click();
		await expect.element(page.getByRole('radio', { name: 'Any' })).toBeChecked();
		await page.getByRole('radio', { name: 'All' }).click();
		await page.getByRole('button', { name: 'Clear' }).click();
		expect(did).toEqual(['all', 'clear']);
	});

	it('shows neither all nor any for a mix of the two', async () => {
		show(['30', '31'], null);
		await heading.click();
		await expect.element(page.getByRole('radio', { name: 'Any' })).not.toBeChecked();
		await expect.element(page.getByRole('radio', { name: 'All' })).not.toBeChecked();
	});

	it('has the server search a type of more tags than a page holds', async () => {
		const did = show([], 'any', 'mood');
		await page.getByRole('button', { name: /^Filter by mood/ }).click();
		const moods = page.getByRole('list', { name: 'Moods' }).getByRole('checkbox');
		await expect.element(moods).toHaveAccessibleName(/Dark/);
		await expect.element(page.getByText('1,499 more. Search to find them.')).toBeVisible();
		await userEvent.fill(page.getByRole('searchbox', { name: 'Find a mood' }), 'calm');
		await expect.element(moods).toHaveAccessibleName(/Calm/);
		await moods.click();
		expect([asked, did]).toEqual([[undefined, 'calm'], ['41']]);
	});

	it('closes on Escape, handing focus back to its heading', async () => {
		show();
		await heading.click();
		await expect.element(page.getByRole('dialog', { name: 'Genre filter' })).toBeVisible();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
		await expect.element(heading).toHaveFocus();
	});
});
