<script lang="ts">
	import { panels } from '$lib/panels.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import type { Repeat } from '$lib/queue/protocol';
	import Icon from './Icon.svelte';
	import Panel from './Panel.svelte';
	import QueueList from './QueueList.svelte';

	// The account's queue: what is coming and what has played, with shuffle and repeat. It shows
	// and edits the account's queue, whichever device plays it (`requirements/queue.md` §2, §7).

	const tabs = [
		{ id: 'up', label: 'Up Next' },
		{ id: 'history', label: 'History' }
	] as const;
	let tab = $state<'up' | 'history'>('up');
	const count = $derived({
		up: queue.counts.manual + queue.counts.upcoming,
		history: queue.counts.history
	});

	/** What each repeat mode turns into when pressed: Off, All, One, then Off again. */
	const NEXT_REPEAT: Record<Repeat, Repeat> = { off: 'all', all: 'one', one: 'off' };

	/** Shuffles Up Next with a seed of this device's choosing, or puts it back in order. */
	function shuffle() {
		const [seed] = crypto.getRandomValues(new Uint32Array(1));
		queue.edit({ type: 'shuffle', on: !queue.shuffled, seed });
	}

	/** Left and Right move between the tabs, as a tab list does. */
	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
		e.preventDefault();
		tab = tab === 'up' ? 'history' : 'up';
		document.getElementById(`queue-tab-${tab}`)?.focus();
	}
</script>

{#if panels.queue}
	<Panel title="Queue" onclose={() => (panels.queue = false)}>
		<div class="queue">
			<div class="bar">
				<div class="tabs" role="tablist" aria-label="Queue" tabindex="-1" {onkeydown}>
					{#each tabs as { id, label } (id)}
						<button
							type="button"
							role="tab"
							id="queue-tab-{id}"
							aria-selected={tab === id}
							tabindex={tab === id ? 0 : -1}
							onclick={() => (tab = id)}
						>
							{label}
							<span class="muted tabular">{count[id].toLocaleString()}</span>
						</button>
					{/each}
				</div>
				<button
					type="button"
					class="toggle"
					class:on={queue.shuffled}
					aria-label="Shuffle"
					aria-pressed={queue.shuffled}
					title="Shuffle"
					disabled={!queue.queueId}
					onclick={shuffle}
				>
					<Icon name="shuffle" size={18} />
				</button>
				<button
					type="button"
					class="toggle"
					class:on={queue.repeat !== 'off'}
					aria-label="Repeat {queue.repeat}"
					title="Repeat {queue.repeat}"
					disabled={!queue.queueId}
					onclick={() => queue.edit({ type: 'repeat', mode: NEXT_REPEAT[queue.repeat] })}
				>
					<Icon name="repeat" size={18} />
					{#if queue.repeat === 'one'}<span class="one" aria-hidden="true">1</span>{/if}
				</button>
			</div>
			<div class="tabpanel" role="tabpanel" aria-labelledby="queue-tab-{tab}">
				<QueueList {tab} />
			</div>
		</div>
	</Panel>
{/if}

<style>
	.queue {
		display: flex;
		flex-direction: column;
		height: 100%;
		gap: var(--space-3);
	}

	.tabpanel {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
	}

	.bar {
		display: flex;
		align-items: center;
		gap: var(--space-1);
	}

	.tabs {
		display: flex;
		flex: 1;
		gap: var(--space-1);
	}

	.tabs button {
		padding: var(--space-2) var(--space-3);
		border: 0;
		border-radius: var(--radius-m);
		background: transparent;
		color: var(--text-muted);
		font: inherit;
		font-size: 14px;
		cursor: pointer;
	}

	.tabs button[aria-selected='true'] {
		background: var(--accent-soft);
		color: var(--accent);
	}

	.toggle {
		position: relative;
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.toggle:hover:not(:disabled) {
		background: var(--surface-hover);
		color: var(--text);
	}

	.toggle:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.toggle.on {
		color: var(--accent);
		background: var(--accent-soft);
	}

	.one {
		position: absolute;
		right: 6px;
		bottom: 5px;
		font-size: 9px;
		font-weight: 700;
	}
</style>
