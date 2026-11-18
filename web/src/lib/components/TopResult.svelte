<script lang="ts">
	// A search's single best match, on its own above the sections and large enough to act on at
	// once: played, queued, or added to a playlist without opening it (`requirements/search.md`
	// §4). It is a shortcut, so it stays in its section too.
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { cardAdding, songs, type Adding } from '$lib/adding.svelte';
	import { listPlaylistItems, listTracks } from '$lib/api/client';
	import type { PlayContext, SearchResult, TrackSummary } from '$lib/api/types';
	import { albumCard, artistCard, playlistCard, tagCard, type Card } from '$lib/cards';
	import { albumTitle } from '$lib/format';
	import { fullText } from '$lib/full-text';
	import { player } from '$lib/player.svelte';
	import { albumRecipe, artistRecipe, playlistRecipe, tagRecipe, type PlayFrom } from '$lib/queue';
	import type { Selected } from '$lib/recent';
	import { tagTypeName } from '$lib/tags';
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
		/**
		 * Playing it is acting on the search's result, which keeps the search: with the result,
		 * where it is a song, an album or an artist, which a recent search can name.
		 */
		onact: (selected?: Selected) => void;
	} = $props();

	const libraryId = $derived(page.data.library?.id ?? '');
	// An album, artist, playlist or tag is what its card is; a track is the song alone.
	const card = $derived.by((): Card | null => {
		if (result.album) return albumCard(result.album);
		if (result.artist) return artistCard(result.artist);
		if (result.playlist) return playlistCard(result.playlist);
		if (result.tag) return tagCard(result.tag);
		return null;
	});
	const track = $derived<TrackSummary | null>(result.track ?? null);
	const adding = $derived<Adding | null>(
		card ? cardAdding(fetch, libraryId, card) : track ? songs([track]) : null
	);
	const kind = $derived.by(() => {
		if (result.tag) return tagTypeName(result.tag.type, page.data.tagTypes ?? []);
		const kinds = { album: 'Album', artist: 'Artist', playlist: 'Playlist', tag: 'Tag' };
		return card ? kinds[card.type] : 'Song';
	});

	let starting = $state(false);

	/**
	 * The first page of `card`'s songs, as its page plays them, what they are played in, and how
	 * the server resolves the rest. The rows lead what the server resolves: a playlist's leave out
	 * the songs the caller hid, as the server does, and keep missing ones in place.
	 */
	async function firstSongs(card: Card): Promise<[TrackSummary[], PlayContext, PlayFrom]> {
		const id = card.id;
		if (card.type === 'playlist') {
			const first = await listPlaylistItems(fetch, libraryId, id);
			const tracks = first.items.flatMap((item) =>
				item.track && !item.track.hidden ? [item.track] : []
			);
			return [tracks, { type: 'playlist', id }, { recipe: playlistRecipe(id), total: card.count }];
		}
		const [query, played, recipe] =
			card.type === 'album'
				? [{ albumId: id }, { type: 'album', id } as const, albumRecipe(id)]
				: card.type === 'artist'
					? [{ artistId: id }, { type: 'artist', id, scope: 'all' } as const, artistRecipe(id)]
					: [{ tagAll: id }, { type: 'tag', id } as const, tagRecipe(id)];
		const first = await listTracks(fetch, libraryId, { ...query, sort: 'album' });
		return [first.items, played, { recipe, total: first.total }];
	}

	/**
	 * A track plays first of the search's songs, as it would from its section. Anything else plays
	 * from its first song, as its page's Play does, once the first page of its songs has loaded;
	 * the server resolves the rest.
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
			const [tracks, played, by] = await firstSongs(card);
			if (!tracks.length) return;
			player.play(tracks, 0, played, by);
			const { type, id } = card;
			onact(type === 'album' || type === 'artist' ? { type, id } : undefined);
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
