// Plugin administration types, shared by the client and the mock: the spec's, and the
// manifest a plugin file carries (`design/plugins.md` §6), which is not part of the API.

import type { components } from './schema';

type Schemas = components['schemas'];

export type PermissionName = Schemas['PermissionName'];
export type PermissionRequest = Schemas['PermissionRequest'];
export type Plugin = Schemas['Plugin'];
export type PluginLibrary = Plugin['libraries'][number];
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
	'scanFinished'
];

export function isLibraryPermission(p: PermissionName): boolean {
	return LIBRARY_PERMISSIONS.includes(p);
}

/** What a plugin file carries (`design/plugins.md`, Packaging). */
export type PluginManifest = {
	id: string;
	name: string;
	version: string;
	apiVersion: string;
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
	searched: {
		title: 'Run when a user who shares their searches settles on one',
		detail:
			'Runs on its own as each user who chose to share their searches settles on one, or removes one.'
	}
};

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
