// Records what this module says about each shared manifest case, so the server's validator in
// crates/plugins, which is tested against the same file, is held to it. Add a case to
// crates/plugins/manifest-cases.json as { "name", "manifest" }, then run:
//
//     node tools/plugin-pack/cases.mjs

import { readFileSync, writeFileSync } from 'node:fs';
import { validateManifest } from './manifest.mjs';

const file = new URL('../../crates/plugins/manifest-cases.json', import.meta.url);
const cases = JSON.parse(readFileSync(file, 'utf8')).map(({ name, manifest }) => ({
	name,
	manifest,
	problems: validateManifest(manifest)
}));
writeFileSync(file, JSON.stringify(cases, null, '\t') + '\n');
console.log(`${cases.length} cases, ${cases.filter((c) => !c.problems.length).length} valid`);
