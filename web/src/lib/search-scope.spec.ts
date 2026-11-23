import { describe, expect, it, vi } from 'vitest';

vi.mock('./api/client', () => ({
	getAlbum: async (_f: unknown, _l: string, id: string) => {
		if (id === '9') throw new Error('not found');
		return { title: null };
	},
	getArtist: async () => ({ name: 'Weezer' }),
	getPlaylist: async () => ({ title: 'Road Trip' }),
	getTag: async () => ({ name: 'Rock' })
}));

const { HELD, containerOf, kindOf, readContainer, scopeParams } = await import('./search-scope');

describe('search scope', () => {
	it('knows the container of a container’s page, and of no other', () => {
		expect(containerOf('/albums/[id]', { id: '31' })).toBe('album:31');
		expect(containerOf('/tags/[id]', { id: '30' })).toBe('tag:30');
		expect(containerOf('/playlists/[id]', { id: '61' })).toBe('playlist:61');
		expect(containerOf('/albums', {})).toBeNull();
		expect(containerOf('/search', {})).toBeNull();
		expect(containerOf(null, {})).toBeNull();
	});

	it('offers a container, and keeps to it only when asked', () => {
		expect(scopeParams('album:31', false)).toEqual({ from: 'album:31' });
		expect(scopeParams('album:31', true)).toEqual({ from: 'album:31', within: 'album:31' });
		expect(scopeParams(null, true)).toEqual({});
	});

	it('knows what each kind of container holds', () => {
		expect(HELD.album).toEqual(['tracks']);
		expect(HELD.tag).toEqual(['albums', 'artists', 'tracks']);
		expect(kindOf('artist:21')).toBe('artist');
		expect(kindOf('song:1')).toBeNull();
	});

	it('names a container as its page does, and none it cannot read', async () => {
		const read = (within: string) => readContainer(fetch, '1', within);
		expect(await read('album:31')).toEqual({
			within: 'album:31',
			kind: 'album',
			name: 'Unknown Album'
		});
		expect(await read('artist:21')).toEqual({
			within: 'artist:21',
			kind: 'artist',
			name: 'Weezer'
		});
		expect((await read('playlist:61'))?.name).toBe('Road Trip');
		expect((await read('tag:30'))?.name).toBe('Rock');
		expect(await read('album:9')).toBeNull();
		expect(await read('song:1')).toBeNull();
		expect(await read('album:')).toBeNull();
	});
});
