<script lang="ts">
	import type { Card } from '$lib/cards';
	import { prefs } from '$lib/prefs.svelte';
	import Artwork from './Artwork.svelte';

	let {
		cards,
		sentinel,
		empty = 'Nothing here yet.'
	}: {
		cards: Card[];
		/** Attached after the last item, to load more as it comes into view. */
		sentinel?: (el: Element) => () => void;
		empty?: string;
	} = $props();
</script>

{#if cards.length === 0}
	<p class="empty muted">{empty}</p>
{:else if prefs.view === 'grid'}
	<ul class="grid">
		{#each cards as card (card.id)}
			<li>
				<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved in cards.ts -->
				<a class="card" href={card.href}>
					<Artwork
						image={card.image}
						seed={card.id}
						size={180}
						icon={card.icon}
						round={card.round}
					/>
					<span class="title">{card.title}</span>
					<span class="subtitle muted">{card.subtitle}</span>
				</a>
			</li>
		{/each}
	</ul>
{:else}
	<ul class="list">
		{#each cards as card (card.id)}
			<li>
				<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved in cards.ts -->
				<a class="row" href={card.href}>
					<span class="thumb">
						<Artwork
							image={card.image}
							seed={card.id}
							size={44}
							icon={card.icon}
							round={card.round}
						/>
					</span>
					<span class="text">
						<span class="title">{card.title}</span>
						<span class="subtitle muted">{card.subtitle}</span>
					</span>
				</a>
			</li>
		{/each}
	</ul>
{/if}
{#if sentinel}
	<div {@attach sentinel} aria-hidden="true"></div>
{/if}

<style>
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: var(--space-5) var(--space-4);
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		padding: var(--space-2);
		margin: calc(-1 * var(--space-2));
		border-radius: var(--radius-m);
	}

	.card:hover,
	.row:hover {
		background: var(--surface-hover);
		text-decoration: none;
	}

	.card :global(.art) {
		margin-bottom: var(--space-1);
		box-shadow: var(--shadow);
	}

	.title {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.subtitle {
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2);
		border-radius: var(--radius-m);
	}

	.thumb {
		width: 44px;
	}

	.text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.empty {
		padding: var(--space-6) 0;
	}

	@media (max-width: 767px) {
		.grid {
			grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
			gap: var(--space-4) var(--space-3);
		}
	}
</style>
