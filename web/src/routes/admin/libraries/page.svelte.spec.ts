import { page } from 'vitest/browser';
import { beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Library, Scan } from '$lib/api/types';

// What the end-to-end tests cannot see on a real server, whose scan of a few files ends within a
// poll: a scan running, and one queued behind it, as the page shows them and asks after them.

const fake = vi.hoisted(() => ({
	/** What the server would list now, for every ask. */
	scans: [] as Scan[],
	started: null as Scan | null,
	invalidated: 0
}));

vi.mock('$lib/api/client', async (original) => ({
	...(await original<object>()),
	adminListScans: async () => ({ items: fake.scans }),
	startScan: async () => fake.started
}));
vi.mock('$app/navigation', () => ({
	invalidateAll: async () => {
		fake.invalidated++;
	}
}));

const { default: Libraries } = await import('./+page.svelte');

const library = {
	id: '1',
	name: 'Music',
	trackCount: 10,
	albumCount: 2,
	artistCount: 1
} as Library;

/** A scan of the whole library, by `trigger`, in `state`, with nothing seen yet. */
function scan(
	id: string,
	trigger: Scan['trigger'],
	state: Scan['state'],
	more: Partial<Scan> = {}
) {
	const finished = state === 'completed' ? '2026-11-11T12:00:00Z' : null;
	return {
		id,
		trigger,
		state,
		startedAt: state === 'queued' ? null : '2026-11-11T11:59:00Z',
		finishedAt: finished,
		filesSeen: 0,
		filesProcessed: 0,
		added: 0,
		updated: 0,
		missing: 0,
		problems: 0,
		wholeLibrary: true,
		currentFolder: null,
		...more
	} satisfies Scan;
}

/** Longer than the page's 1 s between asks. */
const POLLED = { timeout: 3000 };

beforeEach(() => {
	fake.scans = [];
	fake.invalidated = 0;
});

it('shows a scan running, queues one behind it, and follows both to the end', async () => {
	const running = scan('1', 'watch', 'running', {
		filesSeen: 10,
		filesProcessed: 4,
		added: 2,
		currentFolder: 'Aurora Lane/Signal'
	});
	fake.scans = [running];
	render(Libraries, { data: { libraries: [library], scans: { '1': [running] } } } as never);
	await expect
		.element(page.getByText('Scan of changed files · Read 4 of 10 files found so far'))
		.toBeVisible();
	await expect.element(page.getByText('In Aurora Lane/Signal')).toBeVisible();
	await expect.element(page.getByText('So far: 2 added')).toBeVisible();
	expect(page.getByText(/^(Last scanned|Not scanned)/).elements()).toHaveLength(0);

	// A press while it runs queues one to follow it.
	const queued = scan('2', 'manual', 'queued');
	fake.started = queued;
	fake.scans = [queued, { ...running, filesProcessed: 8 }];
	await page.getByRole('button', { name: 'Scan now: Music' }).click();
	await expect.element(page.getByText('Up next: Manual scan')).toBeVisible();
	await expect.element(page.getByRole('button', { name: 'Queued: Music' })).toBeDisabled();
	await expect.element(page.getByText(/Read 8 of 10 files/), POLLED).toBeVisible();

	// The first ends and the queued one starts: the page keeps asking, and reloads nothing yet.
	fake.scans = [scan('2', 'manual', 'running'), scan('1', 'watch', 'completed', { added: 2 })];
	await expect.element(page.getByText('Manual scan · Looking for files'), POLLED).toBeVisible();
	expect(page.getByText(/^Up next/).elements()).toHaveLength(0);
	// The last to finish is not said while another runs.
	expect(page.getByText(/^Last scanned/).elements()).toHaveLength(0);
	await expect.element(page.getByRole('button', { name: 'Scan now: Music' })).toBeEnabled();
	expect(fake.invalidated).toBe(0);

	// With nothing under way, it says how the last scan went and reloads the library's counts.
	fake.scans = [scan('2', 'manual', 'completed')];
	await expect.element(page.getByText(/^Last scanned .*: nothing changed$/), POLLED).toBeVisible();
	expect(page.getByText('Manual scan ·').elements()).toHaveLength(0);
	expect(fake.invalidated).toBe(1);
});
