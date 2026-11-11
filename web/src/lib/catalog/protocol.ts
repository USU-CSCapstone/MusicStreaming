// What the catalog worker and the page say to each other, and what tabs say to each other.
// The worker keeps the device's copy of the libraries current and answers queries of it; the
// page tells it what the realtime connection heard. Only the tab that owns the catalog has a
// worker (`client.svelte.ts`); the others reach it through `router.ts`.

import type { RealtimeServerMessage } from '$lib/api/types';
import type { Call } from '$lib/sync/mutation';
import type { Kind } from './kinds';

/**
 * The version of what tabs say to each other. Every message carries it, and a tab ignores a
 * message of another version, which an older or newer build of the app left open sends: its
 * owner is then no owner to this tab, and the other way round. The lock's name stays the same
 * across versions, so two builds never open the pool together.
 */
export const PROTOCOL = 2;

/** What the owner's worker says of one library on the device. */
export type LibraryState = {
	/** The library has a file that can answer. */
	onDevice: boolean;
	/** The file is part-way through applying a catch-up, so online reads go to the server. */
	applying: boolean;
	/**
	 * Its snapshot was not downloaded because the device has less room than it needs, both in bytes.
	 */
	noRoom?: { need: number; free: number };
};

/** What the owner tells every tab of the catalog, and every tab holds. */
export type CatalogState = {
	protocol: typeof PROTOCOL;
	/** The account the owner's files are for. */
	user: string;
	/** The owner's worker is open and answering; false once the owner is letting go. */
	serving: boolean;
	/**
	 * The owner's worker is inside one long call, a snapshot install, and answers nothing until it
	 * ends.
	 */
	heavy: boolean;
	libraries: Record<string, LibraryState>;
};

/** From the page. */
export type ToWorker =
	/** Loads the module and says where the API is and who is signed in; sent first. */
	| { type: 'init'; api: string; wasm: string; user: string }
	/** Opens the files and pulls; answered with `started`. */
	| { type: 'start' }
	/** A message from the realtime connection. */
	| { type: 'message'; message: RealtimeServerMessage }
	/** Whether the device is working offline, when the worker makes no request; off until said. */
	| { type: 'offline'; value: boolean }
	/** Closes the files and releases the pool to the next owner; answered with `paused`. */
	| { type: 'pause' }
	/** A request for the owner to answer, such as a query; answered with `answer` of `id`. */
	| ({ id: number } & Ask);

/** From the worker. */
export type FromWorker =
	/** Something went wrong that a person may need to know, such as a full disk. */
	| { type: 'error'; message: string; quota: boolean }
	/** The files are open, or could not be. */
	| {
			type: 'started';
			/**
			 * The libraries that had a database file at start, deleted or kept; absent if the start
			 * failed or there is no OPFS.
			 */
			found?: string[];
	  }
	/** What each library on the device stands at now, sent whenever it changes. */
	| { type: 'state'; libraries: Record<string, LibraryState> }
	/**
	 * The library's rows on the device changed, in these kinds: a catch-up was committed (what it
	 * changed) or a snapshot swapped in (everything).
	 */
	| { type: 'changed'; libraryId: string; kinds: readonly Kind[] }
	/** The worker is about to enter, or has left, a call that holds it for seconds. */
	| { type: 'heavy'; on: boolean }
	/** The pool is released. */
	| { type: 'paused' }
	/** The answer to the request of `id`: the JSON of a query, empty for a sign-out. */
	| { type: 'answer'; id: number; result?: string; error?: string };

/** What the owner's page asks its worker: a request from any tab, or whether it is busy. */
export type Ask =
	| Request
	/** Whether a snapshot is downloading, which a hand-over would throw away; answered 1 or empty. */
	| { type: 'busy' };

/** What a tab asks the catalog's owner. */
export type Request =
	/**
	 * Answers the API operation `name` for the library from its file on the device; `offline`
	 * waits for an apply under way rather than failing at once. `user` is who the asking tab is
	 * signed in as; an owner started for someone else gives the catalog up instead of answering.
	 */
	| {
			type: 'query';
			library: string;
			name: string;
			params: object;
			offline?: boolean;
			user?: string;
	  }
	/**
	 * Makes a mutation's writes to the library's file that it does not hold, after any apply under
	 * way (`$lib/sync`); answered empty, also where the library is not on the device. With
	 * `answered`, the server has the mutation: none is made, and those the file holds are made again
	 * through every rebase until the account feed brings them. `user` is as a query's: an owner
	 * started for someone else makes nothing.
	 */
	| { type: 'mutate'; library: string; calls: Call[]; answered?: boolean; user?: string }
	/**
	 * Takes back the device's own writes to the library's file and makes those the log holds again;
	 * answered empty once done. `user` is as a query's.
	 */
	| { type: 'rebase'; library: string; user?: string }
	/**
	 * The server answered a write to the playlist with it at `version`: the library is rebased if
	 * the device made the write over the server's own rows of it; answered empty once done. `user`
	 * is as a query's.
	 */
	| { type: 'confirmed'; library: string; playlistId: string; version: number; user?: string }
	/** Deletes every file on the device. */
	| { type: 'signOut' };
