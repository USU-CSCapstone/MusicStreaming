// Plugin administration types, shared by the client and the mock. Hand-written until
// `api/openapi.yaml` gains permissions: the spec's `Plugin`, plus what the plugin asks
// for and what the admin granted (`requirements/plugins.md` §4).

export type PermissionName = 'libraryRead' | 'libraryWrite' | 'network' | 'listeningActivity';

/** Granted per library. The rest are granted once per plugin (`requirements/plugins.md` §4.1). */
export const LIBRARY_PERMISSIONS: readonly PermissionName[] = ['libraryRead', 'libraryWrite'];

export function isLibraryPermission(p: PermissionName): boolean {
	return LIBRARY_PERMISSIONS.includes(p);
}

export type PermissionRequest = {
	permission: PermissionName;
	required: boolean;
	/** The author's reason, shown to the admin beside the request. */
	reason: string;
	/** For network: host names, or `*` for any. */
	destinations?: string[];
};

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

export type PluginLibrary = {
	libraryId: string;
	enabled: boolean;
	autoDisabled: boolean;
	disabledReason: string | null;
	/** Library permissions granted for this library. */
	granted: PermissionName[];
	/** Required permissions not yet granted: while any remain, it cannot be enabled here. */
	missingRequired: PermissionName[];
};

export type Plugin = Omit<PluginManifest, 'permissions' | 'apiVersion'> & {
	apiVersion: string;
	source: { kind: 'file' | 'url'; url?: string };
	installedAt: string;
	updatedAt?: string;
	/** What it asks for. */
	permissions: PermissionRequest[];
	/** Plugin-wide permissions granted. */
	granted: PermissionName[];
	libraries: PluginLibrary[];
};

/** The body of `PUT /admin/plugins/{pluginId}/permissions`. */
export type PermissionGrants = {
	granted: PermissionName[];
	libraries: { libraryId: string; granted: PermissionName[] }[];
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

/** What `POST /admin/plugins/{pluginId}/run` answers: the plugin's own words, and its log. */
export type PluginRunResult = {
	ok: boolean;
	summary: string;
	log: string[];
	/** Files it saved into the library. */
	saved: number;
	/** Whether the server's scanner is running to pick up what it saved. */
	scannerRunning: boolean;
};
