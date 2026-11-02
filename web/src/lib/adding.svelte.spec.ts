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

	it('has no recipe for a playlist, which goes as its songs', () => {
		expect(cardAdding(fetch, '11', card('playlist')).recipe).toBeUndefined();
	});
});
