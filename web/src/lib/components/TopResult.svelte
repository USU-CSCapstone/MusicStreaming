<script lang="ts">
	// A search's single best match, on its own above the sections and large enough to act on at
	// once: played, queued, or added to a playlist without opening it (`requirements/search.md`
	// §4). It is a shortcut, so it stays in its section too.
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { cardAdding, songs, type Adding } from '$lib/adding.svelte';
	import { listTracks } from '$lib/api/client';
	import type { PlayContext, SearchResult, TrackSummary } from '$lib/api/types';
	import { albumCard, artistCard, type Card } from '$lib/cards';
	import { albumTitle } from '$lib/format';
	import { fullText } from '$lib/full-text';
	import { player } from '$lib/player.svelte';
	import { albumRecipe, artistRecipe, type PlayFrom } from '$lib/queue';
	import type { Selected } from '$lib/recent';
	import { toast } from '$lib/toast.svelte';
	import AddToPlaylistButton from './AddToPlaylistButton.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';

	let {
		result,
		context,
		from,
		onact
	}: {
		result: SearchResult;
		/** What a track plays in: the search, as its section plays. */
		context: PlayContext;
		/** The search's songs in its ranking, which a track plays as the first of. */
		from?: PlayFrom;
		/** Playing it is acting on the search's result, which keeps the search. */
		onact: (selected: Selected) => void;
	} = $props();

	const libraryId = $derived(page.data.library?.id ?? '');
	// An album or an artist is what its card is; a track is the song alone.
	const card = $derived<Card | null>(
		result.album ? albumCard(result.album) : result.artist ? artistCard(result.artist) : null
	);
	const track = $derived<TrackSummary | null>(result.track ?? null);
	const adding = $derived<Adding | null>(
		card ? cardAdding(fetch, libraryId, card) : track ? songs([track]) : null
	);
	const kind = $derived(result.album ? 'Album' : result.artist ? 'Artist' : 'Song');

	let starting = $state(false);

	/**
	 * A track plays first of the search's songs, as it would from its section. An album or an
	 * artist plays from its first song, as its page's Play does, once the first page of its songs
	 * has loaded; the server resolves the rest.
	 */
	async function play() {
		if (track) {
			player.play([track], 0, context, from);
			onact({ type: 'track', id: track.id });
			return;
		}
		if (!card || starting) return;
		starting = true;
		try {
			const id = card.id;
			const [query, played, recipe] =
				card.type === 'album'
					? [{ albumId: id }, { type: 'album', id } as const, albumRecipe(id)]
					: [{ artistId: id }, { type: 'artist', id, scope: 'all' } as const, artistRecipe(id)];
			const first = await listTracks(fetch, libraryId, { ...query, sort: 'album' });
			if (!first.items.length) return;
			player.play(first.items, 0, played, { recipe, total: first.total });
			onact({ type: card.type as 'album' | 'artist', id });
		} catch {
			toast(`Couldn't play ${card.title}.`);
		} finally {
			starting = false;
		}
	}
</script>

<section class="top" aria-label="Top result">
	<div class="art">
		{#if card}
			<Artwork image={card.image} seed={card.id} size={120} icon={card.icon} round={card.round} />
		{:else if track}
			<Artwork
				image={track.album?.image ?? null}
				seed={track.album?.id ?? track.id}
				size={120}
				icon="song"
			/>
		{/if}
	</div>
	<div class="text">
		<span class="kind muted">{kind}</span>
		{#if card}
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved in cards.ts -->
			<a class="title" href={card.href} {@attach fullText}>{card.title}</a>
			<span class="subtitle muted" {@attach fullText}>{card.subtitle}</span>
		{:else if track}
			<span class="title" {@attach fullText}>{track.title}</span>
			<span class="subtitle muted">
				<ArtistLinks artists={track.artists} />
				{#if track.album}
					<span class="sep">·</span>
					<a class="album" href={resolve('/albums/[id]', { id: track.album.id })} {@attach fullText}
						>{albumTitle(track.album.title)}</a
					>
				{/if}
			</span>
		{/if}
		<div class="buttons">
			<button
				type="button"
				class="play"
				aria-label="Play {card?.title ?? track?.title ?? ''}"
				disabled={starting}
				onclick={play}
			>
				<Icon name="play" size={16} /> Play
			</button>
			{#if adding}<AddToPlaylistButton what={adding} />{/if}
		</div>
	</div>
</section>

<style>
	.top {
		display: flex;
		align-items: center;
		gap: var(--space-5);
		margin-bottom: var(--space-6);
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-l);
		background: var(--surface);
	}

	.art {
		width: 120px;
		flex-shrink: 0;
	}

	.text {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-1);
		min-width: 0;
	}

	.kind {
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	.title {
		max-width: 100%;
		overflow: hidden;
		color: var(--text);
		font-size: 24px;
		font-weight: 700;
		text-decoration: none;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	a.title:hover,
	.album:hover {
		text-decoration: underline;
	}

	.subtitle {
		display: flex;
		align-items: baseline;
		gap: var(--space-1);
		max-width: 100%;
		overflow: hidden;
		white-space: nowrap;
	}

	.album {
		overflow: hidden;
		color: inherit;
		text-overflow: ellipsis;
	}

	.buttons {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}

	.play {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
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

	.play:disabled {
		cursor: progress;
		opacity: 0.7;
	}

	@media (max-width: 767px) {
		.top {
			gap: var(--space-4);
			padding: var(--space-3);
		}

		.art {
			width: 88px;
		}

		.title {
			font-size: 20px;
		}
	}
</style>
