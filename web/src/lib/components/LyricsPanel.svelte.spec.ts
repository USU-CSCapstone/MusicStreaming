import { page } from 'vitest/browser';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Lyrics, TrackSummary } from '$lib/api/types';

const lyrics = vi.hoisted(() => ({
	current: { kind: 'none', lines: null, plain: null } as Lyrics
}));

vi.mock('$app/state', () => ({ page: { data: { library: { id: '11' } } } }));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getLyrics: async () => lyrics.current
}));

const { panels } = await import('$lib/panels.svelte');
const { player } = await import('$lib/player.svelte');
const { default: LyricsPanel } = await import('./LyricsPanel.svelte');

const track = {
	id: '41',
	title: 'Brain Stew',
	artists: [{ id: '21', name: 'Green Day' }],
	album: { id: '31', title: 'Insomniac', artists: [], image: null },
	durationUs: 193_000_000,
	explicit: false,
	audio: { codec: 'mp3', lossless: false },
	availability: 'available'
} as TrackSummary;

function open(l: Lyrics) {
	lyrics.current = l;
	player.tracks = [track];
	player.index = 0;
	player.position = 0;
	player.duration = 193; // Seeks clamp to the duration a loaded track reports.
	panels.lyrics = true;
	render(LyricsPanel);
}

afterEach(() => {
	panels.lyrics = false;
});

describe('LyricsPanel.svelte', () => {
	it('highlights the line being sung, and seeks when a line is clicked', async () => {
		open({
			kind: 'synced',
			plain: null,
			lines: [
				{ startMs: 13_000, text: "I'm having trouble trying to sleep" },
				{ startMs: 19_000, text: "I'm counting sheep but running out" },
				{ startMs: 40_000, text: '' }
			]
		});
		const first = page.getByRole('button', { name: "I'm having trouble trying to sleep" });
		await expect.element(first).toBeVisible();
		await expect.element(first).not.toHaveAttribute('aria-current');

		player.position = 20;
		await expect
			.element(page.getByRole('button', { name: "I'm counting sheep but running out" }))
			.toHaveAttribute('aria-current', 'true');

		await first.click();
		expect(player.position).toBe(13);
		await expect.element(first).toHaveAttribute('aria-current', 'true');

		// An empty line is an instrumental break, shown as a note.
		await expect.element(page.getByRole('button', { name: '♪' })).toBeVisible();
	});

	it('shows plain lyrics as text', async () => {
		open({ kind: 'plain', lines: null, plain: 'First line\nSecond line' });
		await expect.element(page.getByText(/First line\s+Second line/)).toBeVisible();
	});

	it('is a clean empty state when a song has none', async () => {
		open({ kind: 'none', lines: null, plain: null });
		await expect.element(page.getByText('No lyrics for this song.')).toBeVisible();
	});

	it('closes from its button', async () => {
		open({ kind: 'none', lines: null, plain: null });
		await page.getByRole('button', { name: 'Close lyrics' }).click();
		expect(panels.lyrics).toBe(false);
	});
});
