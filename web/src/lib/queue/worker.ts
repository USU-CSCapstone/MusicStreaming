// The queue's web worker: a thin shell around `QueueStore`, so loading and editing a queue of
// any size is off the page's thread (`design/web.md`, Queue worker). `client.svelte.ts` is the
// page's side; `protocol.ts` is what they say.

import init from '$lib/core-wasm/jewelcase_core_wasm.js';
import { CatalogRouter } from '$lib/catalog/router';
import { gated } from '$lib/gated-fetch';
import { browserEnv } from '$lib/sync/store';
import { QueueStore } from './store';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** Loading the core takes a moment; what the page sends meanwhile waits behind it, in order. */
let store: Promise<QueueStore> | undefined;

/** The device is working offline: no request leaves until it is not. */
let workingOffline = false;

/**
 * Starts the store on `init`; `offline` takes effect at once, and every other message waits for the
 * store, then goes to it.
 */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	if (data.type === 'offline') {
		workingOffline = data.value;
	} else if (data.type === 'init') {
		store = init({ module_or_path: data.wasm }).then(async () => {
			const ready = new QueueStore({
				api: data.api,
				fetch: gated(() => workingOffline),
				catalog: new CatalogRouter(),
				user: data.user,
				log: { enqueue: (mutation) => post({ type: 'enqueue', mutation }) },
				database: browserEnv(),
				changed: (summary) => post({ type: 'state', summary }),
				refused: (refusal) => post({ type: 'refused', refusal }),
				told: (message) => post({ type: 'notice', message }),
				kept: (entries) => post({ type: 'kept', entries })
			});
			// The queue as the app left it is shown before any other message is handled: each waits on
			// `store`.
			await ready.resume();
			return ready;
		});
	} else {
		store?.then((ready) => {
			if (data.type === 'message') ready.message(data.message);
			else if (data.type === 'edit') ready.edit(data.edit, data.length, data.offline);
			else if (data.type === 'start')
				post({ type: 'started', id: data.id, ok: ready.start(data.request) });
			else if (data.type === 'hidden') ready.hidden(data.libraryId, data.trackId, data.hidden);
			else if (data.type === 'settled') ready.settled(data.mutation, data.outcome, data.result);
			else if (data.type === 'save')
				void ready.save().then(() => post({ type: 'saved', id: data.id }));
			else if (data.type === 'restore')
				void ready
					.restore(data.queueId, data.offline, data.parked)
					.then((result) => post({ type: 'restored', id: data.id, result }));
			else if (data.type === 'deleteKept') ready.deleteKept(data.queueId);
			else post({ type: 'rows', id: data.id, window: ready.rows(data) });
		});
	}
};
