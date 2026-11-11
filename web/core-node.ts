// Loads the core's and the catalog's WebAssembly under node (the unit tests), where `init()`
// cannot fetch a file URL. `src/lib/track-sort.ts` awaits `init()`, which returns at once once
// this has run, so import this before it.
import { readFileSync } from 'node:fs';
import { initSync } from './src/lib/core-wasm/jewelcase_core_wasm.js';
import { initSync as initCatalogSync } from './src/lib/catalog-wasm/jewelcase_catalog_wasm.js';

initSync({
	module: readFileSync(new URL('./src/lib/core-wasm/jewelcase_core_wasm_bg.wasm', import.meta.url))
});

initCatalogSync({
	module: readFileSync(
		new URL('./src/lib/catalog-wasm/jewelcase_catalog_wasm_bg.wasm', import.meta.url)
	)
});
