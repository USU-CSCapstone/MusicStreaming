import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { SentEdit } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	queue: {
		queueId: 'q1' as string | null,
		revision: 1,
		current: null as string | null,
		counts: { manual: 0, upNext: 0, history: 0 },
		repeat: 'off',
		shuffled: false,
		refusal: null as unknown,
		edit: (() => {}) as (edit: SentEdit) => void,
		rows: async (): Promise<{ rows: unknown[] }> => ({ rows: [] })
	},
	player: {
		setShuffle: (() => {}) as (on: boolean) => void,
		setRepeat: (() => {}) as (mode: string) => void,
		jump: () => {},
		rewind: () => {}
	}
}));

vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));
vi.mock('$lib/player.svelte', () => ({ player: fake.player }));

const queue = $state(fake.queue);
Object.assign(fake, { queue });

const { panels } = await import('$lib/panels.svelte');
const { placing } = await import('$lib/queueing.svelte');
const { default: QueuePanel } = await import('./QueuePanel.svelte');

beforeEach(() => {
	vi.restoreAllMocks();
	Object.assign(fake.queue, { queueId: 'q1', repeat: 'off', shuffled: false, refusal: null });
	fake.queue.counts = { manual: 0, upNext: 0, history: 0 };
	panels.queue = true;
});

describe('QueuePanel', () => {
	it('shuffles and steps through the repeat modes', async () => {
		const shuffle = vi.spyOn(fake.player, 'setShuffle');
		const repeat = vi.spyOn(fake.player, 'setRepeat');
		render(QueuePanel);
		await page.getByRole('button', { name: 'Shuffle' }).click();
		expect(shuffle).toHaveBeenCalledWith(true);
		await page.getByRole('button', { name: 'Repeat off' }).click();
		expect(repeat).toHaveBeenLastCalledWith('all');
		fake.queue.repeat = 'all';
		await page.getByRole('button', { name: 'Repeat all' }).click();
		expect(repeat).toHaveBeenLastCalledWith('one');
		fake.queue.repeat = 'one';
		await page.getByRole('button', { name: 'Repeat one' }).click();
		expect(repeat).toHaveBeenLastCalledWith('off');
	});

	it('turns shuffle off when it is on, says so, and is disabled with no queue', async () => {
		const shuffle = vi.spyOn(fake.player, 'setShuffle');
		fake.queue.shuffled = true;
		render(QueuePanel);
		const button = page.getByRole('button', { name: 'Shuffle' });
		await expect.element(button).toHaveAttribute('aria-pressed', 'true');
		await button.click();
		expect(shuffle).toHaveBeenCalledWith(false);
		fake.queue.queueId = null;
		await expect.element(button).toBeDisabled();
	});

	it('shows why an edit did not stay, until it is dismissed', async () => {
		render(QueuePanel);
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
		fake.queue.refusal = {
			code: 'anchor_gone',
			message: 'The queue refused (anchor_gone).',
			edit: null
		};
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent(/the song it was beside has played/);
		await page.getByRole('button', { name: 'Dismiss' }).click();
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
		// The next refusal shows again, and an unknown code shows the queue's own words.
		fake.queue.refusal = { code: 'odd', message: 'Something odd.', edit: null };
		await expect.element(page.getByRole('alert')).toHaveTextContent('Something odd.');
	});

	it('says in plain words that a rewind to a play that is gone did not stay', async () => {
		render(QueuePanel);
		fake.queue.refusal = { code: 'play_gone', message: 'play_gone', edit: null };
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent('That song is no longer in History.');
	});

	it('switches between Up Next, History and Kept by click and by arrow keys', async () => {
		fake.queue.counts = { manual: 0, upNext: 3, history: 2 };
		render(QueuePanel);
		const up = page.getByRole('tab', { name: /Up Next/ });
		const history = page.getByRole('tab', { name: /History/ });
		const keptTab = page.getByRole('tab', { name: 'Kept' });
		await expect.element(page.getByRole('tablist', { name: 'Queue' })).toBeVisible();
		await expect.element(up).toHaveAttribute('aria-selected', 'true');
		await expect.element(history).toHaveTextContent('2');
		await expect.element(page.getByRole('list', { name: 'Up Next' })).toBeVisible();

		up.element().focus();
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(history).toHaveAttribute('aria-selected', 'true');
		await expect.element(up).toHaveAttribute('aria-selected', 'false');
		await expect.element(up).toHaveAttribute('tabindex', '-1');
		await expect.element(history).toHaveAttribute('tabindex', '0');
		expect(document.activeElement).toBe(history.element());
		await expect.element(page.getByRole('list', { name: 'History' })).toBeVisible();
		await expect
			.element(page.getByRole('tabpanel'))
			.toHaveAttribute('aria-labelledby', 'queue-tab-history');
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(keptTab).toHaveAttribute('aria-selected', 'true');
		await expect
			.element(page.getByRole('tabpanel'))
			.toHaveAttribute('aria-labelledby', 'queue-tab-kept');
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(up).toHaveAttribute('aria-selected', 'true');
		await userEvent.keyboard('{End}');
		await expect.element(keptTab).toHaveAttribute('aria-selected', 'true');
		await userEvent.keyboard('{Home}');
		await expect.element(up).toHaveAttribute('aria-selected', 'true');
		await history.click();
		await expect.element(history).toHaveAttribute('aria-selected', 'true');
	});

	it('shows Up Next while choosing where to add, and forgets it when closed', async () => {
		render(QueuePanel);
		await page.getByRole('tab', { name: /History/ }).click();
		await expect
			.element(page.getByRole('tab', { name: /History/ }))
			.toHaveAttribute('aria-selected', 'true');
		placing.what = { label: '“Signal”', count: 1, refs: [], tracks: async () => [] };
		await expect
			.element(page.getByRole('tab', { name: /Up Next/ }))
			.toHaveAttribute('aria-selected', 'true');
		await expect.element(page.getByRole('group', { name: 'Choose where to add' })).toBeVisible();
		panels.queue = false;
		await expect.poll(() => placing.what).toBeNull();
	});

	it('is not drawn while closed', async () => {
		panels.queue = false;
		render(QueuePanel);
		await expect.element(page.getByRole('button', { name: 'Shuffle' })).not.toBeInTheDocument();
	});
});
