import { describe, expect, it } from 'vitest';
import type { Playlist } from './api/types';
import { LIKED_HUE, playlistCard } from './cards';

const playlist = {
	id: 'p1',
	libraryId: '1',
	kind: 'saved',
	title: 'Road Trip',
	trackCount: 2,
	image: { id: 'm1-2-3-4', placeholder: null }
} as Playlist;

describe('playlistCard', () => {
	it('shows a playlist’s mosaic, and Liked Songs a heart in the accent', () => {
		expect(playlistCard(playlist)).toMatchObject({ image: playlist.image, icon: 'playlist' });
		expect(playlistCard(playlist).hue).toBeUndefined();
		const liked = playlistCard({ ...playlist, kind: 'liked', title: 'Liked Songs' });
		expect(liked).toMatchObject({ image: null, icon: 'liked', hue: LIKED_HUE });
	});
});
