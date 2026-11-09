// What the catalog worker and the page say to each other, and what tabs say to each other.
// The worker keeps the device's copy of the libraries current and answers queries of it; the
// page tells it what the realtime connection heard. Only the tab that owns the catalog has a
// worker (`client.svelte.ts`); the others reach it through `router.ts`.

import type { RealtimeServerMessage } from '$lib/api/types';

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
	| { type: 'started' }
	/** The library's rows on the device changed: a catch-up was committed or a snapshot swapped in. */
	| { type: 'changed'; libraryId: string }
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
	/** Whether the library has a file on the device that can answer; answered 1 or empty. */
	| { type: 'onDevice'; library: string }
	/** Whether the library's search index is built on the device as well; answered 1 or empty. */
	| { type: 'indexed'; library: string }
	/** Builds the library's search index in the background, unless built; answered empty at once. */
	| { type: 'prepareIndex'; library: string }
	/** Deletes every file on the device. */
	| { type: 'signOut' };
