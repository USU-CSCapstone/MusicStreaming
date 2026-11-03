// Loads the core's WebAssembly under node (the unit tests and the e2e fixtures), where `init()`
// cannot fetch a file URL. `src/lib/sorting.ts` awaits `init()`, which returns at once once this
// has run, so import this before it.
import { readFileSync } from 'node:fs';
import { initSync } from './src/lib/core-wasm/jewelcase_core_wasm.js';

initSync({
	module: readFileSync(new URL('./src/lib/core-wasm/jewelcase_core_wasm_bg.wasm', import.meta.url))
});
