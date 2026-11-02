<script lang="ts">
	import { panels } from '$lib/panels.svelte';
	import { player } from '$lib/player.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import type { Refusal, Repeat } from '$lib/queue/protocol';
	import Icon from './Icon.svelte';
	import Panel from './Panel.svelte';
	import QueueList from './QueueList.svelte';

	// The account's queue: what is coming, with shuffle and repeat. It shows and edits the
	// account's queue, whichever device plays it (`requirements/queue.md` §2, §7). Shuffle and
	// Repeat are here at every width, since the player bar hides them on a phone.

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
				<h3 class="tab">
					Up Next
					<span class="muted tabular">{queue.counts.upNext.toLocaleString()}</span>
				</h3>
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
			<QueueList />
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

	.tab {
		flex: 1;
		margin: 0;
		font-size: 14px;
		font-weight: 600;
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
