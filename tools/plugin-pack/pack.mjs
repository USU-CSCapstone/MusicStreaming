#!/usr/bin/env node
// Embeds a manifest into a plugin component, producing the one file an admin installs. It checks
// only that the manifest is a JSON object; the server checks what it says, at install.
//
//     node tools/plugin-pack/pack.mjs plugin.wasm manifest.json -o plugin.jc.wasm

import { readFile, writeFile } from 'node:fs/promises';
import { PluginFileError, readManifest, withManifest } from './manifest.mjs';

const args = process.argv.slice(2);
const outAt = args.indexOf('-o');
const out = outAt >= 0 ? args.splice(outAt, 2)[1] : undefined;
const [wasmPath, manifestPath] = args;
if (!wasmPath || !manifestPath || !out) {
	console.error('usage: pack.mjs <component.wasm> <manifest.json> -o <plugin.wasm>');
	process.exit(2);
}

try {
	const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
	const packed = withManifest(new Uint8Array(await readFile(wasmPath)), manifest);
	readManifest(packed); // Refuses a manifest that is not a JSON object.
	await writeFile(out, packed);
	const named = [manifest.name, manifest.version].filter((s) => typeof s === 'string').join(' ');
	console.log(`${out}: ${named ? `${named}, ` : ''}${(packed.length / 1e3).toFixed(0)} KB`);
} catch (e) {
	console.error(e instanceof PluginFileError || e instanceof SyntaxError ? e.message : e);
	process.exit(1);
}
