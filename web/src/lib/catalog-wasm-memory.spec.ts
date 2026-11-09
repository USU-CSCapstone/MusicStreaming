// The search index's memory, in a module of its own: WebAssembly memory only grows, so what a
// build leaves it at is the most it ever used, and other tests' allocations would blur that.

import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';
import { Catalog, initSync } from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';

test('catalog-wasm builds a search index in little more memory than it keeps, and lets the old one go before a rebuild and on request', () => {
	const { memory } = initSync({
		module: readFileSync(new URL('./catalog-wasm/jewelcase_catalog_wasm_bg.wasm', import.meta.url))
	});
	const catalog = Catalog.openInMemory();
	// 200,000 artists of two words each, one of them their own, as large libraries name tracks.
	catalog.sql(`
		WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200000)
		INSERT INTO artists (id, library_id, name, sort_key, added_at)
		SELECT i, 1, printf('Artist %08x', (i * 2654435761) % 4294967296), x'00', 0 FROM n`);
	/** The artist section's total from a search, which needs the index built. */
	const found = () =>
		JSON.parse(catalog.query('1', '1', 'search', JSON.stringify({ q: 'artist', sectionLimit: 1 })))
			.sections[0].total;
	const before = memory.buffer.byteLength;
	catalog.buildIndex('1');
	const built = memory.buffer.byteLength;
	expect(found()).toBe(200_000);
	// The index keeps about 9 MB; the build peaks at under twice that (an index of a value per
	// word and per document peaked at over 50 MB).
	expect(built - before).toBeLessThan(24 << 20);
	// Rebuilds reuse what the old index let go: holding it through each would take its 9 MB more.
	for (let i = 0; i < 4; i++) catalog.buildIndex('1');
	expect(memory.buffer.byteLength - built).toBeLessThan(6 << 20);
	catalog.dropIndex();
	expect(() => found()).toThrow('search index is not built');
	catalog.free();
});
