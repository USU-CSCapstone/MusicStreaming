// The plugin pack tool's reading and writing of the manifest a plugin file carries
// (`design/plugins.md` §6), which plugin authors pack with and the e2e test plugin is built with.

import { describe, expect, it } from 'vitest';
import { readManifest, withManifest } from '../../../tools/plugin-pack/manifest.mjs';
import type { PluginManifest } from './api/plugins';

/** The smallest valid component: the header, and nothing in it. */
const EMPTY_COMPONENT = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]);

const manifest: PluginManifest = {
	id: 'lrclib-lyrics',
	name: 'LRCLIB Lyrics',
	version: '0.1.0',
	apiVersion: '0.1',
	permissions: [
		{ permission: 'libraryRead', required: true, reason: 'To find tracks without lyrics.' },
		{
			permission: 'network',
			required: true,
			reason: 'To fetch lyrics.',
			destinations: ['lrclib.net']
		},
		{ permission: 'libraryAdd', required: false, reason: 'To save .lrc files.' }
	]
};

/** `EMPTY_COMPONENT` with `m` packed in as its manifest. */
const packed = (m: unknown = manifest) => withManifest(EMPTY_COMPONENT, m);

/** The message `readManifest` rejects `bytes` with; fails the test if it accepts them. */
function invalid(bytes: Uint8Array): string {
	try {
		readManifest(bytes);
	} catch (e) {
		return (e as Error).message;
	}
	throw new Error('expected the file to be rejected');
}

describe('plugin files', () => {
	it('round-trips a manifest', () => {
		expect(readManifest(packed())).toEqual(manifest);
	});

	it('replaces rather than duplicates a manifest when repacked', () => {
		const again = withManifest(packed(), { ...manifest, version: '0.2.0' });
		expect(readManifest(again).version).toBe('0.2.0');
	});

	it('rejects what is not a plugin', () => {
		expect(invalid(new TextEncoder().encode('hello'))).toMatch(/not a WebAssembly file/);
		expect(invalid(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0, 0, 0]))).toMatch(
			/module, not a component/
		);
		expect(invalid(EMPTY_COMPONENT)).toMatch(/no Jewelcase manifest/);
	});

	it('rejects a manifest that is not JSON', () => {
		const name = new TextEncoder().encode('jewelcase:manifest');
		const body = [name.length, ...name, ...new TextEncoder().encode('{nope')];
		const bytes = new Uint8Array([...EMPTY_COMPONENT, 0x00, body.length, ...body]);
		expect(invalid(bytes)).toMatch(/not valid JSON/);
	});

	it('leaves checking what a manifest says to the server', () => {
		const bad = { ...manifest, id: 'Bad ID', permissions: [{ permission: 'everything' }] };
		expect(readManifest(packed(bad))).toEqual(bad);
		for (const m of [[manifest], null, 'x']) {
			expect(invalid(packed(m)), JSON.stringify(m)).toMatch(/not a JSON object/);
		}
	});
});
