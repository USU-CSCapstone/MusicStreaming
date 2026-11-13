// The registry: every kind of mutation the app makes, by the name the log keeps it under. A feature
// adds its own here (`design/web.md`, The sync framework, how to declare a mutation). A name is
// never reused for other arguments: a log written by an older build is sent by a newer one.

import type { Definition } from '../mutation';
import { mark } from './marks';
import {
	appendToPlaylist,
	createPlaylist,
	deletePlaylist,
	editPlaylistMutation
} from './playlists';
import { play } from './plays';
import {
	deleteKeptQueueMutation,
	editQueueMutation,
	restoreQueueMutation,
	startQueueMutation
} from './queue';
import { clearSearches, deleteSearch, recordSearch } from './searches';

/** Every kind of mutation, by name. */
export const registry = {
	play,
	mark,
	createPlaylist,
	deletePlaylist,
	editPlaylist: editPlaylistMutation,
	appendToPlaylist,
	startQueue: startQueueMutation,
	editQueue: editQueueMutation,
	restoreQueue: restoreQueueMutation,
	deleteKeptQueue: deleteKeptQueueMutation,
	recordSearch,
	deleteSearch,
	clearSearches
};

/** The name of a kind of mutation. */
export type Name = keyof typeof registry;

/** The arguments a mutation of `name` takes. */
export type Args<N extends Name> = (typeof registry)[N] extends Definition<infer A> ? A : never;

/** The definition of `name`, if this build has it: a newer build's log may name one it has not. */
export const definition = (name: string): Definition<unknown> | undefined =>
	(registry as unknown as Record<string, Definition<unknown>>)[name];
