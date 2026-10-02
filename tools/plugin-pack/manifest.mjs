// The plugin file format (design/plugins.md, Packaging): one WebAssembly component
// whose manifest is a top-level custom section named `jewelcase:manifest`, holding
// UTF-8 JSON. The server's `crates/plugins` is the authority on what installs; this
// module validates the same way for authors, the web mock, and its tests, and both are
// held to `crates/plugins/manifest-cases.json`.

export const SECTION = 'jewelcase:manifest';
export const API_VERSION = '0.2';

/** Permissions a plugin can ask for (requirements/plugins.md §4.1). */
export const PERMISSIONS = [
	'libraryRead',
	'libraryAdd',
	'libraryChange',
	'network',
	'listeningActivity',
	// Hooks, approved like permissions: when tracks are added, changed, or removed; when a scan
	// finishes; and at an interval.
	'tracksChanged',
	'scanFinished',
	'schedule'
];

/** The shortest interval a schedule may ask for. */
export const MIN_EVERY_MINUTES = 5;

const MAGIC = [0x00, 0x61, 0x73, 0x6d];
/** Component-model binaries: version 0x0d, layer 1. A core module is version 1, layer 0. */
const COMPONENT_VERSION = [0x0d, 0x00, 0x01, 0x00];
const ID = /^[a-z0-9][a-z0-9-]{1,63}$/;

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
export function sections(bytes) {
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
 * The manifest a plugin file carries, validated.
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
	const problems = validateManifest(manifest);
	if (problems.length) throw new PluginFileError(`The manifest is invalid: ${problems.join('; ')}.`);
	return manifest;
}

/**
 * Every problem with a manifest, in words an admin can act on. Empty when it is valid.
 * @param {unknown} m
 * @returns {string[]}
 */
export function validateManifest(m) {
	if (typeof m !== 'object' || m === null || Array.isArray(m)) return ['it must be a JSON object'];
	const o = /** @type {Record<string, unknown>} */ (m);
	const problems = [];
	const text = (/** @type {string} */ key, required = true) => {
		const v = o[key];
		if (v === undefined && !required) return;
		if (typeof v !== 'string' || !v.trim()) problems.push(`"${key}" must be a non-empty string`);
	};
	if (typeof o.id !== 'string' || !ID.test(o.id)) {
		problems.push('"id" must be lowercase letters, digits, and hyphens (2–64 characters)');
	}
	text('name');
	text('version');
	text('description', false);
	text('author', false);
	if (o.homepage !== undefined && !/^https?:\/\//.test(typeof o.homepage === 'string' ? o.homepage : '')) {
		problems.push('"homepage" must be an http(s) URL');
	}
	if (o.apiVersion !== API_VERSION) problems.push(`"apiVersion" must be "${API_VERSION}"`);
	for (const key of ['settings', 'personalSettings']) {
		if (o[key] !== undefined) problems.push(...validateSettings(key, o[key]));
	}
	// A run sees both under one set of names.
	const names = (/** @type {string} */ key) => /** @type {any} */ (o[key])?.properties;
	const [admin, personal] = [names('settings'), names('personalSettings')];
	if (isObject(admin) && isObject(personal)) {
		for (const name of Object.keys(personal).sort()) {
			if (Object.hasOwn(admin, name)) problems.push(`personalSettings: "${name}" is also one of "settings"`);
		}
	}
	if (!Array.isArray(o.permissions)) {
		problems.push('"permissions" must be a list (empty if it needs none)');
		return problems;
	}
	const seen = new Set();
	o.permissions.forEach((p, i) => {
		const at = `permissions[${i}]`;
		if (typeof p !== 'object' || p === null || Array.isArray(p)) {
			return problems.push(`${at} must be an object`);
		}
		const r = /** @type {Record<string, unknown>} */ (p);
		if (!PERMISSIONS.includes(/** @type {string} */ (r.permission))) {
			return problems.push(`${at}: unknown permission ${JSON.stringify(r.permission)}`);
		}
		if (seen.has(r.permission)) problems.push(`${at}: ${r.permission} is requested twice`);
		seen.add(r.permission);
		if (typeof r.required !== 'boolean') problems.push(`${at}: "required" must be true or false`);
		if (typeof r.reason !== 'string' || !r.reason.trim()) {
			problems.push(`${at}: every permission needs a "reason" shown to the admin`);
		}
		if (r.permission === 'network') {
			const d = r.destinations;
			if (!Array.isArray(d) || d.length === 0 || d.some((x) => typeof x !== 'string' || !x)) {
				problems.push(`${at}: network needs "destinations", host names or ["*"] for any`);
			}
		} else if (r.destinations !== undefined) {
			problems.push(`${at}: only network takes "destinations"`);
		}
		if (r.permission === 'schedule') {
			const m = r.everyMinutes;
			if (!Number.isInteger(m) || /** @type {number} */ (m) < MIN_EVERY_MINUTES || /** @type {number} */ (m) > 4294967295) {
				problems.push(
					`${at}: schedule needs "everyMinutes", a whole number of minutes, at least ${MIN_EVERY_MINUTES}`
				);
			}
		} else if (r.everyMinutes !== undefined) {
			problems.push(`${at}: only schedule takes "everyMinutes"`);
		}
	});
	// A library hook's event is about the library, which only reading it can make anything of.
	for (const hook of ['tracksChanged', 'scanFinished']) {
		if (seen.has(hook) && !seen.has('libraryRead')) {
			problems.push(`${hook} needs libraryRead as well`);
		}
	}
	return problems;
}

const KINDS = {
	string: { accepts: (/** @type {unknown} */ v) => typeof v === 'string', described: 'text' },
	number: { accepts: (/** @type {unknown} */ v) => typeof v === 'number', described: 'a number' },
	integer: { accepts: (/** @type {unknown} */ v) => Number.isInteger(v), described: 'a whole number' },
	boolean: { accepts: (/** @type {unknown} */ v) => typeof v === 'boolean', described: 'true or false' }
};

/** @param {unknown} v */
const isObject = (v) => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * Every problem with the settings a manifest declares under `key`: a small part of JSON Schema,
 * in the same words as the server's `crates/plugins/src/settings.rs`.
 * @param {string} key
 * @param {unknown} settings
 * @returns {string[]}
 */
function validateSettings(key, settings) {
	const props = /** @type {Record<string, unknown>} */ (settings)?.properties;
	if (typeof settings !== 'object' || settings === null || !isObject(props)) {
		return [`"${key}" must be an object with "properties"`];
	}
	const properties = /** @type {Record<string, unknown>} */ (props);
	const problems = [];
	// Sorted, as the server's map of them is.
	for (const name of Object.keys(properties).sort()) {
		if (!/^[A-Za-z][A-Za-z0-9_]{0,63}$/.test(name)) {
			problems.push(`${key}: "${name}" is not a setting name (letters, digits, and _, starting with a letter)`);
			continue;
		}
		const at = `${key}.${name}`;
		const s = /** @type {Record<string, unknown>} */ (properties[name]);
		if (typeof s !== 'object' || s === null || Array.isArray(s)) {
			problems.push(`${at} must be an object`);
			continue;
		}
		const kind = KINDS[/** @type {keyof typeof KINDS} */ (s.type)];
		if (typeof s.type !== 'string' || !Object.hasOwn(KINDS, s.type)) {
			problems.push(`${at}: "type" must be string, number, integer, or boolean`);
			continue;
		}
		for (const key of ['title', 'description']) {
			const v = s[key];
			if (v !== undefined && !(typeof v === 'string' && v.trim())) {
				problems.push(`${at}: "${key}" must be a non-empty string`);
			}
		}
		if (s.writeOnly !== undefined && typeof s.writeOnly !== 'boolean') {
			problems.push(`${at}: "writeOnly" must be true or false`);
		}
		if (s.enum !== undefined && !(Array.isArray(s.enum) && s.enum.length && s.enum.every(kind.accepts))) {
			problems.push(`${at}: "enum" must be a non-empty list of ${s.type} values`);
		}
		if (s.default !== undefined && !kind.accepts(s.default)) {
			problems.push(`${at}: "default" must be ${kind.described}`);
		}
	}
	const required = /** @type {Record<string, unknown>} */ (settings).required;
	if (required !== undefined && !(Array.isArray(required) && required.every((n) => typeof n === 'string' && Object.hasOwn(properties, n)))) {
		problems.push(`${key}: "required" must be a list of its settings' names`);
	}
	return problems;
}

/**
 * The component with `manifest` embedded, replacing any manifest it already had.
 * @param {Uint8Array} bytes
 * @param {unknown} manifest
 * @returns {Uint8Array}
 */
export function withManifest(bytes, manifest) {
	const problems = validateManifest(manifest);
	if (problems.length) throw new PluginFileError(`The manifest is invalid: ${problems.join('; ')}.`);
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
