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
export const LIBRARY_PERMISSIONS: readonly PermissionName[] = ['libraryRead', 'libraryWrite'];

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
	libraryWrite: {
		title: 'Write to the library',
		detail: 'Can create, change, and delete files in this library.'
	},
	network: { title: 'Network access', detail: 'Reach services outside this server.' },
	listeningActivity: {
		title: 'Listening activity',
		detail: 'What is played, and listening history.'
	}
};
