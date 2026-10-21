// The plugin file format (design/plugins.md §6): one WebAssembly component whose manifest is a
// top-level custom section named `jewelcase:manifest`, holding UTF-8 JSON. Nothing here checks
// what the manifest says: the server's `crates/plugins` does that, at install.

const SECTION = 'jewelcase:manifest';

const MAGIC = [0x00, 0x61, 0x73, 0x6d];
/** Component-model binaries: version 0x0d, layer 1. A core module is version 1, layer 0. */
const COMPONENT_VERSION = [0x0d, 0x00, 0x01, 0x00];

export class PluginFileError extends Error {}

/**
 * @param {Uint8Array} bytes
 * @param {number} at
 * @returns {[value: number, next: number]}
 */
function leb(bytes, at) {
	let value = 0;
	let shift = 0;
	for (;;) {
		if (at >= bytes.length) throw new PluginFileError('The file ends in the middle of a section.');
		const b = bytes[at++];
		value += (b & 0x7f) * 2 ** shift;
		if ((b & 0x80) === 0) return [value, at];
		shift += 7;
		if (shift > 35) throw new PluginFileError('The file has a malformed section length.');
	}
}

/** @param {number} n */
function encodeLeb(n) {
	const out = [];
	do {
		let b = n & 0x7f;
		n = Math.floor(n / 128);
		if (n > 0) b |= 0x80;
		out.push(b);
	} while (n > 0);
	return out;
}

/**
 * The component's top-level sections, without descending into nested modules.
 * @param {Uint8Array} bytes
 * @returns {{ id: number, start: number, end: number, name?: string, payload?: Uint8Array }[]}
 */
function sections(bytes) {
	if (bytes.length < 8 || MAGIC.some((b, i) => bytes[i] !== b)) {
		throw new PluginFileError('This is not a WebAssembly file.');
	}
	if (COMPONENT_VERSION.some((b, i) => bytes[4 + i] !== b)) {
		throw new PluginFileError(
			'This is a WebAssembly module, not a component. Plugins are built as components.'
		);
	}
	const out = [];
	let at = 8;
	while (at < bytes.length) {
		const start = at;
		const id = bytes[at++];
		const [size, body] = leb(bytes, at);
		const end = body + size;
		if (end > bytes.length) throw new PluginFileError('The file is truncated.');
		if (id === 0) {
			const [nameLen, nameAt] = leb(bytes, body);
			const name = new TextDecoder().decode(bytes.subarray(nameAt, nameAt + nameLen));
			out.push({ id, start, end, name, payload: bytes.subarray(nameAt + nameLen, end) });
		} else {
			out.push({ id, start, end });
		}
		at = end;
	}
	return out;
}

/**
 * The manifest a plugin file carries, refused only when it is not a JSON object.
 * @param {Uint8Array} bytes
 * @returns {Record<string, unknown>}
 */
export function readManifest(bytes) {
	const found = sections(bytes).filter((s) => s.name === SECTION);
	if (found.length === 0) {
		throw new PluginFileError(
			'This component has no Jewelcase manifest. Pack it with tools/plugin-pack first.'
		);
	}
	if (found.length > 1) throw new PluginFileError('This component has more than one manifest.');
	let manifest;
	try {
		manifest = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(found[0].payload));
	} catch {
		throw new PluginFileError('The manifest is not valid JSON.');
	}
	if (typeof manifest !== 'object' || manifest === null || Array.isArray(manifest)) {
		throw new PluginFileError('The manifest is not a JSON object.');
	}
	return manifest;
}

/**
 * The component with `manifest` embedded, replacing any manifest it already had.
 * @param {Uint8Array} bytes
 * @param {unknown} manifest
 * @returns {Uint8Array}
 */
export function withManifest(bytes, manifest) {
	const kept = sections(bytes).filter((s) => s.name !== SECTION);
	const name = new TextEncoder().encode(SECTION);
	const payload = new TextEncoder().encode(JSON.stringify(manifest));
	const body = [...encodeLeb(name.length), ...name, ...payload];
	const section = Uint8Array.from([0x00, ...encodeLeb(body.length), ...body]);
	const parts = [bytes.subarray(0, 8), ...kept.map((s) => bytes.subarray(s.start, s.end)), section];
	const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
	let at = 0;
	for (const p of parts) {
		out.set(p, at);
		at += p.length;
	}
	return out;
}
