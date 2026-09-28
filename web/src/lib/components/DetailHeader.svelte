<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { ImageRef } from '$lib/api/types';
	import Artwork from './Artwork.svelte';
	import Icon, { type IconName } from './Icon.svelte';

	let {
		kind,
		title,
		image,
		seed,
		icon,
		round = false,
		onplay,
		children
	}: {
		/** "Album", "Artist", "Playlist" — shown above the title. */
		kind: string;
		title: string;
		image: ImageRef | null;
		seed: string;
		icon: IconName;
		round?: boolean;
		/** Plays the page's songs from the top; omitted when there is nothing to play. */
		onplay?: () => void;
		/** Credits and details under the title. */
		children?: Snippet;
	} = $props();
</script>

<header>
	<div class="art">
		<Artwork {image} {seed} size={200} {icon} {round} />
	</div>
	<div class="text">
		<span class="kind muted">{kind}</span>
		<h1>{title}</h1>
		{#if children}<div class="meta">{@render children()}</div>{/if}
		{#if onplay}
			<button type="button" class="play" onclick={onplay}>
				<Icon name="play" size={16} /> Play
			</button>
		{/if}
	</div>
</header>

<style>
	header {
		display: flex;
		align-items: flex-end;
		gap: var(--space-5);
		margin-bottom: var(--space-6);
	}

	.art {
		width: 200px;
		flex-shrink: 0;
	}

	.art :global(.art) {
		box-shadow: var(--shadow);
	}

	.text {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-2);
		min-width: 0;
	}

	.kind {
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	h1 {
		font-size: 36px;
		letter-spacing: -0.015em;
		overflow-wrap: anywhere;
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		color: var(--text-muted);
	}

	.play {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-2);
		height: 38px;
		padding: 0 var(--space-5) 0 var(--space-4);
		border: 0;
		border-radius: 19px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	.play:hover {
		filter: brightness(1.08);
	}

	@media (max-width: 767px) {
		header {
			flex-direction: column;
			align-items: center;
			text-align: center;
			gap: var(--space-4);
		}

		.art {
			width: min(200px, 60vw);
		}

		.text {
			align-items: center;
		}

		h1 {
			font-size: 26px;
		}

		.meta {
			align-items: center;
		}
	}
</style>
