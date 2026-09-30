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

/** Granted per library. The rest are granted once per plugin (`requirements/plugins.md` §4.1). */
export const LIBRARY_PERMISSIONS: readonly PermissionName[] = [
	'libraryRead',
	'libraryAdd',
	'libraryChange',
	'tracksChanged'
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
	}
};
