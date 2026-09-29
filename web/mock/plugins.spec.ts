import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { PluginManifest } from '../src/lib/api/plugins';
import { readManifest, withManifest } from '../../tools/plugin-pack/manifest.mjs';
import { PluginError, PluginStore, adminRoute } from './plugins';

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
		{ permission: 'libraryWrite', required: false, reason: 'To save .lrc files.' }
	]
};

const packed = (m: unknown = manifest) => withManifest(EMPTY_COMPONENT, m);

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

	it('names what is wrong with an invalid manifest', () => {
		const bad = {
			...manifest,
			id: 'Bad ID',
			permissions: [
				{ permission: 'everything', required: true, reason: 'x' },
				{ permission: 'network', required: true, reason: '' }
			]
		};
		expect(() => packed(bad)).toThrow(
			/"id".*unknown permission "everything".*needs a "reason".*destinations/
		);
	});
});

describe('PluginStore', () => {
	let dir: string;
	let store: PluginStore;
	const LIB = '11';

	beforeEach(() => {
		dir = mkdtempSync(join(tmpdir(), 'jc-plugins-'));
		store = new PluginStore(dir, () => [LIB]);
	});
	afterEach(() => rmSync(dir, { recursive: true, force: true }));

	const grantAll = () =>
		store.setPermissions('lrclib-lyrics', {
			granted: ['network'],
			libraries: [{ libraryId: LIB, granted: ['libraryRead', 'libraryWrite'] }]
		});

	it('installs disabled, with nothing granted', () => {
		const p = store.install(packed(), { kind: 'file' });
		expect(p.granted).toEqual([]);
		expect(p.libraries).toEqual([
			expect.objectContaining({
				libraryId: LIB,
				enabled: false,
				granted: [],
				missingRequired: ['libraryRead', 'network']
			})
		]);
	});

	it('refuses a second install of the same plugin', () => {
		store.install(packed(), { kind: 'file' });
		expect(() => store.install(packed(), { kind: 'file' })).toThrow(PluginError);
	});

	it('cannot be enabled until its required permissions are granted', () => {
		store.install(packed(), { kind: 'file' });
		expect(() => store.setEnabled('lrclib-lyrics', LIB, true)).toThrow(
			/still needs Read the library and Network access/
		);
		store.setPermissions('lrclib-lyrics', {
			granted: ['network'],
			libraries: [{ libraryId: LIB, granted: ['libraryRead'] }]
		});
		expect(store.setEnabled('lrclib-lyrics', LIB, true).libraries[0].enabled).toBe(true);
	});

	it('ignores grants for permissions it never asked for, or in the wrong scope', () => {
		store.install(packed(), { kind: 'file' });
		const p = store.setPermissions('lrclib-lyrics', {
			granted: ['network', 'listeningActivity', 'libraryRead'],
			libraries: [
				{ libraryId: LIB, granted: ['libraryWrite', 'network'] },
				{ libraryId: 'other', granted: ['libraryRead'] }
			]
		});
		expect(p.granted).toEqual(['network']);
		expect(p.libraries[0].granted).toEqual(['libraryWrite']);
	});

	it('disables at once when a required permission is revoked', () => {
		store.install(packed(), { kind: 'file' });
		grantAll();
		store.setEnabled('lrclib-lyrics', LIB, true);
		const p = store.setPermissions('lrclib-lyrics', {
			granted: [],
			libraries: [{ libraryId: LIB, granted: ['libraryRead'] }]
		});
		expect(p.libraries[0]).toMatchObject({ enabled: false, missingRequired: ['network'] });
		expect(p.libraries[0].disabledReason).toMatch(/revoked/);
	});

	it('keeps grants across uninstall and reinstall, but not the enabled state', () => {
		store.install(packed(), { kind: 'file' });
		grantAll();
		store.setEnabled('lrclib-lyrics', LIB, true);
		store.uninstall('lrclib-lyrics');
		expect(store.list()).toEqual([]);
		const again = store.install(packed(), { kind: 'file' });
		expect(again.granted).toEqual(['network']);
		expect(again.libraries[0]).toMatchObject({
			enabled: false,
			granted: ['libraryRead', 'libraryWrite']
		});
	});

	it('persists to disk', () => {
		store.install(packed(), { kind: 'file' });
		grantAll();
		expect(new PluginStore(dir, () => [LIB]).get('lrclib-lyrics').granted).toEqual(['network']);
	});

	it('answers as Problems over the admin routes', async () => {
		const bytes = packed();
		const ok = await adminRoute(store, 'POST', '/admin/plugins', 'application/octet-stream', bytes);
		expect(ok?.status).toBe(201);
		const dup = await adminRoute(
			store,
			'POST',
			'/admin/plugins',
			'application/octet-stream',
			bytes
		);
		expect(dup).toMatchObject({ status: 409, body: { code: 'plugin_exists' } });
		const early = await adminRoute(
			store,
			'PUT',
			`/admin/plugins/lrclib-lyrics/libraries/${LIB}`,
			'application/json',
			new TextEncoder().encode('{"enabled":true}')
		);
		expect(early).toMatchObject({ status: 409, body: { code: 'permissions_required' } });
		const bad = await adminRoute(
			store,
			'POST',
			'/admin/plugins',
			'application/octet-stream',
			EMPTY_COMPONENT
		);
		expect(bad).toMatchObject({ status: 422, body: { code: 'plugin_invalid' } });
		expect(
			await adminRoute(store, 'DELETE', '/admin/plugins/lrclib-lyrics', '', new Uint8Array())
		).toEqual({ status: 204 });
		expect(
			(await adminRoute(store, 'GET', '/admin/plugins/nope', '', new Uint8Array()))?.status
		).toBe(404);
	});
});
