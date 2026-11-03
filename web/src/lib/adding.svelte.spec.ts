import { describe, expect, it } from 'vitest';
import { cardAdding } from './adding.svelte';
import type { Card } from './cards';

const card = (type: Card['type']): Card => ({
	type,
	count: 7,
	id: '5',
	href: '/x',
	title: 'Signal',
	subtitle: '',
	image: null,
	icon: type,
	round: false
});

describe('cardAdding', () => {
	it('names an album and an artist by the recipe the server resolves, in album order', () => {
		expect(cardAdding(fetch, '11', card('album'))).toMatchObject({
			label: '“Signal”',
			count: 7,
			refs: [{ type: 'album', id: '5' }],
			recipe: { type: 'album', albumId: '5', sort: 'album', order: 'asc' }
		});
		expect(cardAdding(fetch, '11', card('artist')).recipe).toEqual({
			type: 'artist',
			artistId: '5',
			scope: 'all',
			artistCredit: 'any',
			sort: 'album',
			order: 'asc'
		});
	});

	it('names a playlist by its recipe, queued by the count the card has and with no fetch', async () => {
		const fetched: string[] = [];
		const spy = ((input: RequestInfo | URL) => {
			fetched.push(String(input));
			return Promise.reject(new Error('no fetch expected'));
		}) as typeof fetch;
		const adding = cardAdding(spy, '11', card('playlist'));
		expect(adding).toMatchObject({
			label: '“Signal”',
			count: 7,
			refs: [{ type: 'playlist', id: '5' }],
			recipe: { type: 'playlist', playlistId: '5' }
		});
		// No sort or order is sent for the playlist's own order, and no version, which is the server's.
		expect(adding.recipe).toEqual({ type: 'playlist', playlistId: '5' });
		expect(fetched).toEqual([]);
	});
});
