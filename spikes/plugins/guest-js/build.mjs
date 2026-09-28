// Builds the JS guest twice: interpreted (the default) and ahead-of-time compiled with weval.
import { componentize } from '@bytecodealliance/componentize-js';
import { writeFile } from 'node:fs/promises';

const common = {
	sourcePath: 'plugin.js',
	witPath: '../wit',
	worldName: 'plugin',
	// Plugins get no network in the spike; stdio stays so errors are visible.
	disableFeatures: ['http', 'fetch-event']
};

for (const [name, enableAot] of [['guest_js.wasm', false], ['guest_js_aot.wasm', true]]) {
	const started = performance.now();
	const { component } = await componentize({ ...common, enableAot });
	await writeFile(name, component);
	const secs = ((performance.now() - started) / 1000).toFixed(1);
	console.log(`${name}: ${(component.byteLength / 1e6).toFixed(1)} MB in ${secs} s`);
}
