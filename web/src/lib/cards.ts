// Albums, artists, and playlists as the one shape collection views render.

import { resolve } from '$app/paths';
import type { AlbumSummary, ArtistSummary, ImageRef, Playlist } from './api/types';
import type { IconName } from './components/Icon.svelte';
import { albumTitle, artistName, plural, releaseYear } from './format';

export type Card = {
	/** What it is, and how many songs it holds, for queueing and dragging it. */
	type: 'album' | 'artist' | 'playlist';
	count: number;
	id: string;
	href: string;
	title: string;
	subtitle: string;
	image: ImageRef | null;
	icon: IconName;
	round: boolean;
	/** The stand-in's colour, where it is fixed rather than picked by the ID. */
	hue?: number;
};

/** The hue of Liked Songs' artwork, the accent's: always a heart, never a mosaic of its songs. */
export const LIKED_HUE = 252;

/**
 * A playlist's artwork: a mosaic of its songs' albums, or for Liked Songs a heart
 * (`requirements/playlists.md` §11).
 */
export function playlistArt(p: Playlist): { image: ImageRef | null; icon: IconName; hue?: number } {
	return p.kind === 'liked'
		? { image: null, icon: 'liked', hue: LIKED_HUE }
		: { image: p.image, icon: 'playlist' };
}

/** The card for an album, with its song count for queueing and dragging. */
export function albumCard(a: AlbumSummary): Card {
	const artists = a.artists.map((x) => artistName(x.name)).join(', ');
	const year = releaseYear(a.releaseDate);
	return {
		type: 'album',
		count: a.trackCount,
		id: a.id,
		href: resolve('/albums/[id]', { id: a.id }),
		title: albumTitle(a.title),
		subtitle: [artists, year].filter(Boolean).join(' · '),
		image: a.image,
		icon: 'album',
		round: false
	};
}

/** The card for an artist, with its song count for queueing and dragging. */
export function artistCard(a: ArtistSummary): Card {
	return {
		type: 'artist',
		count: a.trackCount,
		id: a.id,
		href: resolve('/artists/[id]', { id: a.id }),
		title: artistName(a.name),
		subtitle: a.albumCount > 0 ? plural(a.albumCount, 'album') : plural(a.trackCount, 'song'),
		image: a.image,
		icon: 'artist',
		round: true
	};
}

/** The card for a playlist, with its song count for queueing and dragging. */
export function playlistCard(p: Playlist): Card {
	return {
		type: 'playlist',
		count: p.trackCount,
		id: p.id,
		href: resolve('/playlists/[id]', { id: p.id }),
		title: p.title,
		subtitle: plural(p.trackCount, 'song'),
		...playlistArt(p),
		round: false
	};
}
