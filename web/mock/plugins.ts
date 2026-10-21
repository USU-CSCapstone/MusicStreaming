// Plugin administration for the mock: install, permissions, enable, uninstall, as the
// server's `/admin/plugins` does. Nothing runs a plugin here; that needs the server
// (`crates/plugins`). State lives under the data directory, never the library.

import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import {
	PERMISSION_LABELS,
	type PermissionGrants,
	type PermissionName,
	type Plugin,
	type PluginLibrary,
	type PluginManifest,
	type PluginRunResult,
	type PluginSettings,
	type SettingSchema
} from '../src/lib/api/plugins.ts';
import { PluginFileError, readManifest } from '../../tools/plugin-pack/manifest.mjs';

const LIBRARY_PERMISSIONS: PermissionName[] = [
	'libraryRead',
	'libraryAdd',
	'libraryChange',
	'tracksChanged',
	'scanFinished',
	'albumsChanged',
	'artistsChanged'
];
const MAX_BYTES = 50 * 1024 * 1024;
/** The server's rule for a plugin id, which the mock names its file by. */
const ID = /^[a-z0-9][a-z0-9-]{1,63}$/;

type Record_ = {
	manifest: PluginManifest;
	source: Plugin['source'];
	installedAt: string;
	granted: PermissionName[];
	libraries: Record<
		string,
		{ enabled: boolean; granted: PermissionName[]; disabledReason: string | null }
	>;
	/** Values set per level: '' for every library, else a library's ID. */
	settings?: Record<string, Record<string, unknown>>;
};

/** Grants kept across uninstall, so a reinstall needs no re-approval (`requirements/plugins.md` §5). */
type Retained = { granted: PermissionName[]; libraries: Record<string, PermissionName[]> };

/** Hosts whose limits for every plugin each plugin is exempt from, kept across uninstall like grants. */
type Index = {
	plugins: Record<string, Record_>;
	retained: Record<string, Retained>;
	exemptions?: Record<string, string[]>;
};

export class PluginError extends Error {
	constructor(
		readonly status: number,
		readonly code:
			| 'runner_unavailable'
			| 'plugin_invalid'
			| 'plugin_exists'
			| 'permissions_required'
			| 'not_found'
			| 'validation_failed',
		message: string
	) {
		super(message);
	}
}

export class PluginStore {
	constructor(
		readonly dir: string,
		/** The libraries that exist right now. */
		private libraries: () => string[]
	) {}

	#index(): Index {
		const path = join(this.dir, 'index.json');
		if (!existsSync(path)) return { plugins: {}, retained: {} };
		return JSON.parse(readFileSync(path, 'utf8'));
	}

	#save(index: Index) {
		mkdirSync(this.dir, { recursive: true });
		writeFileSync(join(this.dir, 'index.json'), JSON.stringify(index, null, '\t'));
	}

	#view(r: Record_, index = this.#index()): Plugin {
		const { manifest: m } = r;
		const exemptions = index.exemptions?.[m.id] ?? [];
		const destinations = m.permissions.find((p) => p.permission === 'network')?.destinations ?? [];
		const reaches = (host: string) =>
			destinations.some((d) => d === '*' || d.toLowerCase() === host);
		const required = m.permissions.filter((p) => p.required).map((p) => p.permission);
		const libraries: PluginLibrary[] = this.libraries().map((libraryId) => {
			const lib = r.libraries[libraryId] ?? { enabled: false, granted: [], disabledReason: null };
			const has = new Set([...r.granted, ...lib.granted]);
			return {
				libraryId,
				enabled: lib.enabled,
				autoDisabled: lib.disabledReason !== null,
				disabledReason: lib.disabledReason,
				granted: lib.granted,
				missingRequired: required.filter((p) => !has.has(p)),
				// The mock never runs a plugin.
				lastRun: null
			};
		});
		return {
			id: m.id,
			name: m.name,
			version: m.version,
			apiVersion: m.apiVersion,
			description: m.description,
			author: m.author,
			homepage: m.homepage,
			source: r.source,
			installedAt: r.installedAt,
			permissions: m.permissions,
			granted: r.granted,
			hasSettings: Object.keys(settingsSchema(m).properties).length > 0,
			libraries,
			hostLimits: inForce(index)
				.filter((l) => reaches(l.host))
				.map((l) => ({ ...l, exempt: exemptions.includes(l.host) }))
		};
	}

	/** The plugin installed under `id`, or a 404 `PluginError`. */
	#get(index: Index, id: string): Record_ {
		// Own keys only, so an id such as `__proto__` is not found on the prototype.
		if (!Object.hasOwn(index.plugins, id))
			throw new PluginError(404, 'not_found', 'No such plugin.');
		return index.plugins[id];
	}

	list(): Plugin[] {
		const index = this.#index();
		return Object.values(index.plugins)
			.sort((a, b) => a.manifest.name.localeCompare(b.manifest.name))
			.map((r) => this.#view(r, index));
	}

	get(id: string): Plugin {
		return this.#view(this.#get(this.#index(), id));
	}

	/** Installed disabled for every library, with any grants it held before (`requirements/plugins.md` §5). */
	install(bytes: Uint8Array, source: Plugin['source']): Plugin {
		if (bytes.length > MAX_BYTES)
			throw new PluginError(422, 'plugin_invalid', 'The file is over 50 MB.');
		let manifest: PluginManifest;
		try {
			manifest = readManifest(bytes) as PluginManifest;
			// The server refuses a manifest without a permissions list; the mock reads it as empty.
			manifest.permissions ??= [];
		} catch (e) {
			if (e instanceof PluginFileError) throw new PluginError(422, 'plugin_invalid', e.message);
			throw e;
		}
		// The rest is the server's to check; the mock needs only what it stores and lists by.
		const { id, name, permissions } = manifest;
		if (
			typeof id !== 'string' ||
			!ID.test(id) ||
			typeof name !== 'string' ||
			!Array.isArray(permissions)
		) {
			throw new PluginError(
				422,
				'plugin_invalid',
				'The manifest needs an "id" of lowercase letters, digits, and hyphens, a "name", and a "permissions" list.'
			);
		}
		const index = this.#index();
		if (Object.hasOwn(index.plugins, manifest.id)) {
			throw new PluginError(409, 'plugin_exists', `${manifest.name} is already installed.`);
		}
		const requested = new Set(manifest.permissions.map((p) => p.permission));
		const kept = Object.hasOwn(index.retained, manifest.id)
			? index.retained[manifest.id]
			: undefined;
		const libraries: Record_['libraries'] = {};
		for (const [libraryId, granted] of Object.entries(kept?.libraries ?? {})) {
			libraries[libraryId] = {
				enabled: false,
				granted: granted.filter((p) => requested.has(p)),
				disabledReason: null
			};
		}
		const record: Record_ = {
			manifest,
			source,
			installedAt: new Date().toISOString(),
			granted: (kept?.granted ?? []).filter((p) => requested.has(p)),
			libraries
		};
		mkdirSync(this.dir, { recursive: true });
		writeFileSync(join(this.dir, `${manifest.id}.wasm`), bytes);
		index.plugins[manifest.id] = record;
		delete index.retained[manifest.id];
		this.#save(index);
		return this.#view(record);
	}

	async installFromUrl(url: string): Promise<Plugin> {
		let parsed: URL;
		try {
			parsed = new URL(url);
		} catch {
			throw new PluginError(422, 'validation_failed', 'That is not a URL.');
		}
		if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
			throw new PluginError(422, 'validation_failed', 'Plugins are fetched over http or https.');
		}
		let res: Response;
		try {
			res = await fetch(parsed, { signal: AbortSignal.timeout(30_000) });
		} catch (e) {
			throw new PluginError(
				422,
				'plugin_invalid',
				`Could not download it: ${(e as Error).message}.`
			);
		}
		if (!res.ok)
			throw new PluginError(422, 'plugin_invalid', `Could not download it: HTTP ${res.status}.`);
		if (Number(res.headers.get('content-length') ?? 0) > MAX_BYTES) {
			throw new PluginError(422, 'plugin_invalid', 'The file is over 50 MB.');
		}
		return this.install(new Uint8Array(await res.arrayBuffer()), { kind: 'url', url: parsed.href });
	}

	/** Files it wrote stay; its grants are kept for a reinstall (`requirements/plugins.md` §5). */
	uninstall(id: string) {
		const index = this.#index();
		const r = this.#get(index, id);
		index.retained[id] = {
			granted: r.granted,
			libraries: Object.fromEntries(Object.entries(r.libraries).map(([k, v]) => [k, v.granted]))
		};
		delete index.plugins[id];
		rmSync(join(this.dir, `${id}.wasm`), { force: true });
		this.#save(index);
	}

	/**
	 * Replaces what is granted. Anything not requested is ignored, and a library that loses
	 * a required permission is disabled at once (`requirements/plugins.md` §4.2).
	 */
	setPermissions(id: string, grants: PermissionGrants): Plugin {
		const index = this.#index();
		const r = this.#get(index, id);
		const requested = new Set(r.manifest.permissions.map((p) => p.permission));
		const pick = (list: unknown, library: boolean) =>
			[...new Set(Array.isArray(list) ? list : [])].filter(
				(p): p is PermissionName => requested.has(p) && LIBRARY_PERMISSIONS.includes(p) === library
			);
		r.granted = pick(grants.granted, false);
		const known = new Set(this.libraries());
		for (const lib of Array.isArray(grants.libraries) ? grants.libraries : []) {
			if (!known.has(lib.libraryId)) continue;
			const current = r.libraries[lib.libraryId] ?? {
				enabled: false,
				granted: [],
				disabledReason: null
			};
			r.libraries[lib.libraryId] = { ...current, granted: pick(lib.granted, true) };
		}
		for (const [libraryId, lib] of Object.entries(r.libraries)) {
			const view = this.#view(r).libraries.find((l) => l.libraryId === libraryId);
			if (lib.enabled && view && view.missingRequired.length) {
				lib.enabled = false;
				lib.disabledReason = 'A required permission was revoked.';
			}
		}
		this.#save(index);
		return this.#view(r);
	}

	/** Exempts it from, or holds it to, the limit for every plugin at one host. */
	setExempt(id: string, host: string, exempt: boolean) {
		if (!isHostName(host)) {
			throw new PluginError(
				422,
				'validation_failed',
				'An exemption is for one host name, such as musicbrainz.org.'
			);
		}
		const index = this.#index();
		this.#get(index, id);
		const lowercase = host.toLowerCase();
		const others = (index.exemptions?.[id] ?? []).filter((h) => h !== lowercase);
		const hosts = exempt ? [...others, lowercase].sort() : others;
		index.exemptions = { ...index.exemptions, [id]: hosts };
		this.#save(index);
	}

	/** What is set at `level` ('' for every library), secrets left out, and which are set. */
	settings(id: string, level: string): PluginSettings {
		const r = this.#get(this.#index(), id);
		const schema = settingsSchema(r.manifest);
		const stored = r.settings?.[level] ?? {};
		const secret = (name: string) => schema.properties[name]?.writeOnly === true;
		// The spec types these JSON objects only as objects.
		return {
			schema: { type: 'object', ...schema },
			values: Object.fromEntries(Object.entries(stored).filter(([k]) => !secret(k))),
			secretsSet: Object.keys(stored).filter(secret)
		} as unknown as PluginSettings;
	}

	/** Saves as the server does: a secret left out keeps its value, `null` clears one. */
	setSettings(id: string, level: string, entered: Record<string, unknown>) {
		const index = this.#index();
		const r = this.#get(index, id);
		const schema = settingsSchema(r.manifest);
		const stored = r.settings?.[level] ?? {};
		const kept = Object.entries(stored).filter(([k]) => schema.properties[k]?.writeOnly);
		const values: Record<string, unknown> = Object.fromEntries(kept);
		for (const [k, v] of Object.entries(entered)) {
			if (v === null) delete values[k];
			else values[k] = v;
		}
		r.settings = { ...r.settings, [level]: values };
		this.#save(index);
	}

	/** Disabling is immediate; enabling needs every required permission (`requirements/plugins.md` §4.2). */
	setEnabled(id: string, libraryId: string, enabled: boolean): Plugin {
		const index = this.#index();
		const r = this.#get(index, id);
		if (!this.libraries().includes(libraryId))
			throw new PluginError(404, 'not_found', 'No such library.');
		const lib = r.libraries[libraryId] ?? { enabled: false, granted: [], disabledReason: null };
		r.libraries[libraryId] = lib;
		if (enabled) {
			const missing = this.#view(r).libraries.find(
				(l) => l.libraryId === libraryId
			)!.missingRequired;
			if (missing.length) {
				const names = missing.map((p) => PERMISSION_LABELS[p].title);
				throw new PluginError(
					409,
					'permissions_required',
					`It still needs ${names.join(' and ')}.`
				);
			}
		}
		lib.enabled = enabled;
		lib.disabledReason = null;
		this.#save(index);
		return this.#view(r);
	}
}

/**
 * The limits every plugin keeps to, by host, as the server works them out: each set by a plugin
 * granted `hostRateLimits` and enabled in a library, the strictest winning.
 */
function inForce(index: Index): { host: string; limit: string; setBy: string[] }[] {
	const limits = new Map<string, { host: string; limit: string; setBy: string[] }>();
	for (const id of Object.keys(index.plugins).sort()) {
		const r = index.plugins[id];
		const enabled = Object.values(r.libraries).some((l) => l.enabled);
		const granted = ['network', 'hostRateLimits'] as const;
		if (!enabled || !granted.every((p) => r.granted.includes(p))) continue;
		for (const request of r.manifest.permissions) {
			for (const [name, declared] of Object.entries(request.rateLimits ?? {})) {
				if (typeof declared === 'string' || declared.per !== 'host') continue;
				const host = name.toLowerCase();
				const kept = limits.get(host);
				const stricter = kept ? gap(declared.limit) - gap(kept.limit) : 1;
				if (stricter > 0) limits.set(host, { host, limit: declared.limit, setBy: [id] });
				else if (stricter === 0) kept!.setBy.push(id);
			}
		}
	}
	return [...limits.values()].sort((a, b) => a.host.localeCompare(b.host));
}

/** The seconds a limit such as "60/min" keeps between requests. */
function gap(limit: string): number {
	const [count, unit] = limit.split('/');
	return ({ s: 1, min: 60, h: 3600 }[unit] ?? 1) / Number(count);
}

/** `s` with ASCII capitals lowercase and nothing else changed, as the server compares hosts. */
const asciiLower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/**
 * Whether `host` is one host name, as the server's `is_host_name` reads it: one the URL parser
 * gives back unchanged but for ASCII case. So a pattern, a URL, a port, a trailing dot, an IPv4
 * shorthand such as `127.1`, IPv6 outside brackets, or an IDN not in punycode never is.
 */
function isHostName(host: string): boolean {
	try {
		const parsed = new URL(`http://${host}/`).hostname;
		return asciiLower(parsed) === asciiLower(host) && !host.includes('*') && !host.endsWith('.');
	} catch {
		return false;
	}
}

/** The settings a manifest declares, or none. */
function settingsSchema(m: PluginManifest): {
	properties: Record<string, SettingSchema>;
	required?: string[];
} {
	return m.settings ?? { properties: {} };
}

export type AdminResponse = { status: number; body?: unknown };

/**
 * `/admin/plugins…` for any transport: the Vite middleware and the e2e fixtures both call
 * this, so tests exercise the same logic the dev server runs.
 */
export async function adminRoute(
	store: PluginStore,
	method: string,
	path: string,
	contentType: string,
	body: Uint8Array,
	/** Runs an installed plugin; injected so tests need no real runner. */
	run?: (id: string) => Promise<PluginRunResult>
): Promise<AdminResponse | undefined> {
	// The path may carry a query, as settings take `libraryId`.
	const url = new URL(path, 'http://mock');
	const query = url.searchParams;
	const m =
		/^\/admin\/plugins(?:\/([^/]+))?(?:\/(permissions|exemptions|libraries|run|settings)(?:\/([^/]+))?)?$/.exec(
			url.pathname
		);
	if (!m) return undefined;
	const [, id, sub, libraryId] = m.map((s) => (s === undefined ? s : decodeURIComponent(s)));
	const json = () => {
		try {
			return JSON.parse(new TextDecoder().decode(body));
		} catch {
			throw new PluginError(422, 'validation_failed', 'The request body is not JSON.');
		}
	};
	try {
		if (!id && method === 'GET') return { status: 200, body: { items: store.list() } };
		if (!id && method === 'POST') {
			if (contentType.startsWith('application/json')) {
				return { status: 201, body: await store.installFromUrl(String(json().url ?? '')) };
			}
			return { status: 201, body: store.install(body, { kind: 'file' }) };
		}
		if (id && !sub && method === 'GET') return { status: 200, body: store.get(id) };
		if (id && !sub && method === 'DELETE') {
			store.uninstall(id);
			return { status: 204 };
		}
		if (id && sub === 'permissions' && method === 'PUT') {
			return { status: 200, body: store.setPermissions(id, json()) };
		}
		if (id && sub === 'exemptions' && libraryId && (method === 'PUT' || method === 'DELETE')) {
			// The path's last part is the host here.
			store.setExempt(id, libraryId, method === 'PUT');
			return { status: 204 };
		}
		if (id && sub === 'libraries' && libraryId && method === 'PUT') {
			return { status: 200, body: store.setEnabled(id, libraryId, json().enabled === true) };
		}
		if (id && sub === 'settings' && (method === 'GET' || method === 'PUT')) {
			const level = query.get('libraryId') ?? '';
			if (method === 'PUT') store.setSettings(id, level, json().values ?? {});
			return { status: 200, body: store.settings(id, level) };
		}
		if (id && sub === 'run' && method === 'POST') {
			store.get(id); // 404 for a plugin that is not installed.
			if (!run) {
				throw new PluginError(
					503,
					'runner_unavailable',
					'Running a plugin needs the Jewelcase server.'
				);
			}
			return { status: 200, body: await run(id) };
		}
		return {
			status: 405,
			body: problem(405, 'Method Not Allowed', 'validation_failed', 'Not supported.')
		};
	} catch (e) {
		if (e instanceof PluginError)
			return { status: e.status, body: problem(e.status, titleFor(e.status), e.code, e.message) };
		throw e;
	}
}

function titleFor(status: number): string {
	return { 404: 'Not Found', 409: 'Conflict', 422: 'Unprocessable Content' }[status] ?? 'Error';
}

function problem(status: number, title: string, code: string, detail: string) {
	return { type: 'about:blank', title, status, code, detail };
}
