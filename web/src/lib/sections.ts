import { resolve } from '$app/paths';
import type { IconName } from './components/Icon.svelte';

export type Section = { href: string; route: string; label: string; icon: IconName };

/** The top-level sections, in sidebar and tab-bar order. */
export const sections: Section[] = [
	{ href: resolve('/albums'), route: '/albums', label: 'Albums', icon: 'album' },
	{ href: resolve('/artists'), route: '/artists', label: 'Artists', icon: 'artist' },
	{ href: resolve('/playlists'), route: '/playlists', label: 'Playlists', icon: 'playlist' },
	{ href: resolve('/songs'), route: '/songs', label: 'Songs', icon: 'song' }
];

/** Whether the route belongs to a section; an album page belongs to Albums. */
export function inSection(routeId: string | null, section: Section): boolean {
	return routeId === section.route || (routeId?.startsWith(`${section.route}/`) ?? false);
}
