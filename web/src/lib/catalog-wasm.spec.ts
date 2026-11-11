import { expect, test } from 'vitest';
import { Catalog } from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';

test('catalog-wasm opens an in-memory database and creates the schema', () => {
	const catalog = Catalog.openInMemory();
	expect(catalog.schemaVersion()).toBe(2);
	expect(catalog.tables()).toEqual(
		expect.arrayContaining([
			'albums',
			'artists',
			'images',
			'tags',
			'tracks',
			'track_tags',
			'playlists',
			'playlist_items'
		])
	);
	catalog.free();
});
