// What the catalog worker and the page say to each other. The worker keeps the device's copy of
// the libraries current; the page tells it what the realtime connection heard.

import type { RealtimeServerMessage } from '$lib/api/types';

/** From the page. */
export type ToWorker =
	/** Loads the module and says where the API is and who is signed in; sent first. */
	| { type: 'init'; api: string; wasm: string; user: string }
	/** Opens the files and pulls. */
	| { type: 'start' }
	/** A message from the realtime connection. */
	| { type: 'message'; message: RealtimeServerMessage }
	/** Deletes every file. */
	| { type: 'signOut' };

/** From the worker. */
export type FromWorker =
	/** Something went wrong that a person may need to know, such as a full disk. */
	| { type: 'error'; message: string; quota: boolean }
	/** Every file is deleted. */
	| { type: 'signedOut' };
