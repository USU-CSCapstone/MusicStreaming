<script lang="ts">
	import { page } from '$app/state';
	import { cardAdding } from '$lib/adding.svelte';
	import type { Card } from '$lib/cards';
	import { canDrag, drop, lift } from '$lib/dragging';
	import { prefs } from '$lib/prefs.svelte';
	import AddToPlaylistButton from './AddToPlaylistButton.svelte';
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

	const libraryId = $derived(page.data.library?.id ?? '');

	/** A mouse drags the card toward the queue panel; its songs are only fetched if it is dropped. */
	const dragCard = (e: DragEvent, card: Card) =>
		lift(e, cardAdding(fetch, libraryId, card), libraryId);
</script>

{#if cards.length === 0}
	<p class="empty muted">{empty}</p>
{:else if prefs.view === 'grid'}
	<ul class="grid">
		{#each cards as card (card.id)}
			<li>
				<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in cards.ts -->
				<a
					class="card"
					href={card.href}
					draggable={canDrag}
					ondragstart={(e) => dragCard(e, card)}
					ondragend={drop}
				>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
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
				<span class="more"><AddToPlaylistButton what={cardAdding(fetch, libraryId, card)} /></span>
			</li>
		{/each}
	</ul>
{:else}
	<ul class="list">
		{#each cards as card (card.id)}
			<li>
				<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in cards.ts -->
				<a
					class="row"
					href={card.href}
					draggable={canDrag}
					ondragstart={(e) => dragCard(e, card)}
					ondragend={drop}
				>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
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
				<span class="more"><AddToPlaylistButton what={cardAdding(fetch, libraryId, card)} /></span>
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

	li {
		position: relative;
	}

	/* Over a card's corner, or at a row's end; shown for the card pointed at, and always where
	   there is no pointing. */
	.more {
		position: absolute;
		opacity: 0;
	}

	.grid .more {
		top: var(--space-2);
		right: var(--space-2);
	}

	.list .more {
		/* Centred on the row, without a transform, which would hold the menu's fixed position. */
		top: calc(50% - 19px);
		right: var(--space-2);
	}

	li:hover .more,
	li:focus-within .more {
		opacity: 1;
	}

	@media (hover: none) {
		.more {
			opacity: 1;
		}
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
