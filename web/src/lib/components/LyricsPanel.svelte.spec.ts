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
const { playback } = await import('$lib/playback.svelte');
const { default: LyricsPanel } = await import('./LyricsPanel.svelte');

const track = {
	id: '41',
	title: 'Brain Stew',
	artists: [{ id: '21', name: 'Green Day' }],
	album: { id: '31', title: 'Insomniac', artists: [], image: null },
	discNumber: null,
	trackNumber: null,
	durationUs: 193_000_000,
	releaseDate: null,
	explicit: false,
	audio: { codec: 'mp3', lossless: false },
	availability: 'available',
	sortKeys: { title: 'brain stew', artist: 'green day', album: 'insomniac' }
} as TrackSummary;

function open(l: Lyrics) {
	lyrics.current = l;
	player.current = track;
	player.position = 0;
	player.duration = 193; // Seeks clamp to the duration a loaded track reports.
	panels.lyrics = true;
	render(LyricsPanel);
}

afterEach(() => {
	panels.lyrics = false;
	playback.state = null;
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

	it('seeks nowhere while playback is stopped', async () => {
		const sung = { kind: 'synced', plain: null, lines: [{ startMs: 13_000, text: 'Sleep' }] };
		open(sung as Lyrics);
		playback.track = track;
		playback.state = {
			status: 'stopped',
			activeDeviceId: null,
			itemId: '0',
			positionMs: 0,
			positionAt: new Date().toISOString(),
			volume: null
		};
		await expect.element(page.getByRole('button', { name: 'Sleep' })).toBeDisabled();
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
