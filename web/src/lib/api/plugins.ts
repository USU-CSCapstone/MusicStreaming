// Plugin administration types, shared by the client and the mock: the spec's, and the
// manifest a plugin file carries (`design/plugins.md` §6), which is not part of the API.

import type { components } from './schema';

type Schemas = components['schemas'];

export type PermissionName = Schemas['PermissionName'];
export type PermissionRequest = Schemas['PermissionRequest'];
export type Plugin = Schemas['Plugin'];
export type PluginLibrary = Plugin['libraries'][number];
/** A rate limit every plugin's requests to a host keep to, at a host this plugin reaches. */
export type PluginHostLimit = Plugin['hostLimits'][number];
/** A destination's rate limit as a manifest gives it: `"1/s"`, or with whose requests it counts. */
export type DeclaredLimit = NonNullable<PermissionRequest['rateLimits']>[string];
/** The body of `PUT /admin/plugins/{pluginId}/permissions`. */
export type PermissionGrants = Schemas['PermissionGrants'];
/** What `POST /admin/plugins/{pluginId}/run` answers: the plugin's own words, and its log. */
export type PluginRunResult = Schemas['PluginRunResult'];
export type PluginSettings = Schemas['PluginSettings'];
/** A plugin a user connects with their own account (`GET /me/plugins`). */
export type PersonalPlugin = Schemas['PersonalPlugin'];

/** One setting, as a plugin's settings schema declares it. */
export type SettingSchema = {
	type: 'string' | 'number' | 'integer' | 'boolean';
	title?: string;
	description?: string;
	enum?: (string | number | boolean)[];
	default?: string | number | boolean;
	writeOnly?: boolean;
};

/** Granted per library. The rest are granted once per plugin (`requirements/plugins.md` §4.1). */
export const LIBRARY_PERMISSIONS: readonly PermissionName[] = [
	'libraryRead',
	'libraryAdd',
	'libraryChange',
	'tracksChanged',
	'scanFinished',
	'albumsChanged',
	'artistsChanged'
];

export function isLibraryPermission(p: PermissionName): boolean {
	return LIBRARY_PERMISSIONS.includes(p);
}

/** What a plugin file carries (`design/plugins.md`, Packaging). */
export type PluginManifest = {
	id: string;
	name: string;
	version: string;
	apiVersion: Plugin['apiVersion'];
	description?: string;
	author?: string;
	homepage?: string;
	permissions: PermissionRequest[];
	settings?: { properties: Record<string, SettingSchema>; required?: string[] };
	personalSettings?: { properties: Record<string, SettingSchema>; required?: string[] };
};

export const PERMISSION_LABELS: Record<PermissionName, { title: string; detail: string }> = {
	libraryRead: { title: 'Read the library', detail: 'Its catalog, artwork, lyrics, and audio.' },
	libraryAdd: {
		title: 'Add files to the library',
		detail: 'Can create new files in this library. It cannot change or delete what is there.'
	},
	libraryChange: {
		title: 'Change or delete files in the library',
		detail: 'Can replace, move, and delete any file in this library, your music included.'
	},
	network: { title: 'Network access', detail: 'Reach services outside this server.' },
	listeningActivity: {
		title: 'Listening activity',
		detail: 'What is played, and listening history.'
	},
	// A hook, approved like a permission: when the plugin runs, rather than what it reaches.
	tracksChanged: {
		title: 'Run when tracks change',
		detail:
			'Runs on its own as tracks are added, changed, or removed, starting with every track already here.'
	},
	scanFinished: {
		title: 'Run when a scan finishes',
		detail: 'Runs on its own each time a scan of this library finishes.'
	},
	schedule: {
		title: 'Run on a schedule',
		detail: 'Runs on its own at a set interval, in every library it is enabled in.'
	},
	played: {
		title: 'Run when a connected user plays something',
		detail: 'Runs on its own as each user who connected it finishes playing a track.'
	},
	playing: {
		title: 'Run when a connected user starts playing something',
		detail:
			'Runs on its own as each user who connected it starts a track, to show what is playing now.'
	},
	searchActivity: {
		title: 'Search activity',
		detail: 'What people search for, from those who choose to share it with this plugin.'
	},
	albumsChanged: {
		title: 'Run when albums change',
		detail:
			'Runs on its own as albums are added, changed, or removed, starting with every album already here.'
	},
	artistsChanged: {
		title: 'Run when artists change',
		detail:
			'Runs on its own as artists are added, changed, or removed, starting with every artist already here.'
	},
	searched: {
		title: 'Run when a user who shares their searches settles on one',
		detail:
			'Runs on its own as each user who chose to share their searches settles on one, or removes one.'
	},
	hostRateLimits: {
		title: 'Limit a service for every plugin',
		detail:
			'Can set a rate limit that every plugin keeps to, for a service that limits this whole server. It can slow other plugins.'
	}
};

const UNITS: Record<string, string> = { s: 'second', min: 'minute', h: 'hour' };

/** A rate limit such as "60/min" in words: "60 per minute". */
export function pace(limit: string): string {
	const [count, unit] = limit.split('/');
	return `${count} per ${UNITS[unit] ?? unit}`;
}

/** A declared limit with whose requests it counts: a bare "1/s" counts the plugin's own. */
function counted(limit: DeclaredLimit): Exclude<DeclaredLimit, string> {
	return typeof limit === 'string' ? { limit, per: 'plugin' } : limit;
}

/** Whether a declared limit is one every plugin keeps to. */
export function forEveryPlugin(limit: DeclaredLimit): boolean {
	return counted(limit).per === 'host';
}

const PER = {
	plugin: 'for this plugin alone',
	user: 'for each connected user',
	host: 'for every plugin'
};

/** A declared limit in words: "1 per second, for every plugin". */
export function describeLimit(declared: DeclaredLimit): string {
	const { limit, per } = counted(declared);
	return `${pace(limit)}, ${PER[per]}`;
}

/** An interval in the largest whole unit: "day", "6 hours", "90 minutes". */
export function every(minutes: number): string {
	const [n, unit] =
		minutes % 1440 === 0
			? [minutes / 1440, 'day']
			: minutes % 60 === 0
				? [minutes / 60, 'hour']
				: [minutes, 'minute'];
	return n === 1 ? unit : `${n} ${unit}s`;
}
