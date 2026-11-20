import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { LyricMatch, PlayContext, TrackSummary } from '$lib/api/types';

const fake = vi.hoisted(() => ({ plays: [] as unknown[][] }));
vi.mock('$lib/player.svelte', () => ({
	player: { play: (...args: unknown[]) => void fake.plays.push(args) }
}));

const { default: LyricMatches } = await import('./LyricMatches.svelte');

const track = (id: string, title: string) =>
	({
		id,
		title,
		artists: [{ id: '21', name: 'Weezer' }],
		album: { id: '31', title: 'Blue Album', artists: [], image: null },
		availability: 'available',
		liked: false
	}) as unknown as TrackSummary;

const matches: LyricMatch[] = [
	{
		track: track('5', "Say It Ain't So"),
		line: { text: "Say it ain't so", startMs: 1200 },
		before: ['Flip on the telly'],
		after: ['Your drug is a heartbreaker']
	},
	{
		track: track('7', 'My Name Is Jonas'),
		line: { text: 'My name is Jonas', startMs: null },
		before: [],
		after: []
	}
];
const context: PlayContext = { type: 'search', query: 'say it', section: 'lyrics' };

beforeEach(() => {
	fake.plays.length = 0;
});

describe('LyricMatches.svelte', () => {
	it('shows each song with its line between the lines around it', async () => {
		render(LyricMatches, { props: { matches, context } });

		const items = page.getByRole('listitem');
		await expect
			.element(items.nth(0))
			.toHaveTextContent(
				/Say It Ain't So\s*Weezer\s*Flip on the telly\s*Say it ain't so\s*Your drug is a heartbreaker/
			);
		await expect
			.element(items.nth(1))
			.toHaveTextContent(/^\s*My Name Is Jonas\s*Weezer\s*My name is Jonas\s*$/);
		await expect.element(page.getByRole('link', { name: 'Weezer' }).first()).toBeVisible();
	});

	it('plays a match’s song from its start, then the songs after it', async () => {
		const played = vi.fn();
		render(LyricMatches, { props: { matches, context, onplay: played } });

		await page.getByRole('button', { name: 'Play My Name Is Jonas' }).click();

		expect(fake.plays).toEqual([[matches.map((match) => match.track), 1, context]]);
		expect(played).toHaveBeenCalledWith(matches[1].track);
	});
});
