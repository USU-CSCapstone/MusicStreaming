<script lang="ts">
	import { panels } from '$lib/panels.svelte';
	import { player } from '$lib/player.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import { placing } from '$lib/queueing.svelte';
	import type { Refusal, Repeat } from '$lib/queue/protocol';
	import Icon from './Icon.svelte';
	import Panel from './Panel.svelte';
	import QueueList from './QueueList.svelte';

	// The account's queue: what is coming and what has played, with shuffle and repeat. It shows
	// and edits the account's queue, whichever device plays it (`requirements/queue.md` §2, §7).
	// Shuffle and Repeat are here at every width, since the player bar hides them on a phone.

	const tabs = [
		{ id: 'upNext', label: 'Up Next' },
		{ id: 'history', label: 'History' }
	] as const;
	let tab = $state<'upNext' | 'history'>('upNext');
	// Choosing where something goes happens in Up Next, so show it; closing the panel gives the choice up.
	$effect(() => {
		if (placing.what) tab = 'upNext';
	});
	$effect(() => {
		if (!panels.queue) placing.what = null;
	});
	const count = $derived({ upNext: queue.counts.upNext, history: queue.counts.history });

	/** Left and Right, Home and End move between the tabs, as a tab list does. */
	function onkeydown(e: KeyboardEvent) {
		const keys: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, Home: -2, End: 2 };
		const step = keys[e.key];
		if (step === undefined || e.altKey || e.ctrlKey || e.metaKey) return;
		e.preventDefault();
		const at = tabs.findIndex((t) => t.id === tab);
		const to = Math.abs(step) === 2 ? (step < 0 ? 0 : tabs.length - 1) : at + step;
		tab = tabs[(to + tabs.length) % tabs.length].id;
		document.getElementById(`queue-tab-${tab}`)?.focus();
	}

	/** What each repeat mode turns into when pressed: Off, All, One, then Off again. */
	const NEXT_REPEAT: Record<Repeat, Repeat> = { off: 'all', all: 'one', one: 'off' };

	/** What a refusal's code means to the person whose edit it was. */
	const WHY: Record<string, string> = {
		anchor_gone: 'That spot has changed: the song it was beside has played or been removed.',
		play_gone: 'That song is no longer in History.',
		superseded: 'Another device started a different queue, so this change did not apply.',
		lost: 'The server no longer has that queue, so this change did not apply.',
		no_queue: 'There is no queue to change.'
	};

	/** The refusal the person has closed, so it shows again only for a later one. */
	let dismissed = $state<Refusal | null>(null);
	const refusal = $derived(queue.refusal && queue.refusal !== dismissed ? queue.refusal : null);
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
							aria-controls="queue-tabpanel"
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
					onclick={() => player.setShuffle(!queue.shuffled)}
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
					onclick={() => player.setRepeat(NEXT_REPEAT[queue.repeat])}
				>
					<Icon name={queue.repeat === 'one' ? 'repeatOne' : 'repeat'} size={18} />
				</button>
			</div>
			{#if refusal}
				<div class="refusal" role="alert">
					<span>{WHY[refusal.code] ?? refusal.message}</span>
					<button type="button" aria-label="Dismiss" onclick={() => (dismissed = refusal)}>
						<Icon name="close" size={14} />
					</button>
				</div>
			{/if}
			<div class="tabpanel" id="queue-tabpanel" role="tabpanel" aria-labelledby="queue-tab-{tab}">
				{#key tab}
					<QueueList {tab} />
				{/key}
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

	.tabpanel {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
	}

	.toggle {
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

	.refusal {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--danger);
		border-radius: var(--radius-m);
		color: var(--danger);
		font-size: 13px;
	}

	.refusal span {
		flex: 1;
	}

	.refusal button {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: inherit;
		cursor: pointer;
	}
</style>
