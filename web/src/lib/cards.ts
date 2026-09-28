// Albums, artists, and playlists as the one shape collection views render.

import { resolve } from '$app/paths';
import type { AlbumSummary, ArtistSummary, ImageRef, Playlist } from './api/types';
import type { IconName } from './components/Icon.svelte';
import { albumTitle, artistName, plural, releaseYear } from './format';

export type Card = {
	id: string;
	href: string;
	title: string;
	subtitle: string;
	image: ImageRef | null;
	icon: IconName;
	round: boolean;
};

export function albumCard(a: AlbumSummary): Card {
	const artists = a.artists.map((x) => artistName(x.name)).join(', ');
	const year = releaseYear(a.releaseDate);
	return {
		id: a.id,
		href: resolve('/albums/[id]', { id: a.id }),
		title: albumTitle(a.title),
		subtitle: [artists, year].filter(Boolean).join(' · '),
		image: a.image,
		icon: 'album',
		round: false
	};
}

export function artistCard(a: ArtistSummary): Card {
	return {
		id: a.id,
		href: resolve('/artists/[id]', { id: a.id }),
		title: artistName(a.name),
		subtitle: a.albumCount > 0 ? plural(a.albumCount, 'album') : plural(a.trackCount, 'song'),
		image: a.image,
		icon: 'artist',
		round: true
	};
}

export function playlistCard(p: Playlist): Card {
	return {
		id: p.id,
		href: resolve('/playlists/[id]', { id: p.id }),
		title: p.title,
		subtitle: plural(p.trackCount, 'song'),
		image: p.image,
		icon: 'playlist',
		round: false
	};
}
