// The queue's web worker: a thin shell around `QueueStore`, so loading and editing a queue of
// any size is off the page's thread (`design/web.md`, Queue worker). `client.svelte.ts` is the
// page's side; `protocol.ts` is what they say.

import init from '$lib/core-wasm/jewelcase_core_wasm.js';
import { CatalogRouter } from '$lib/catalog/router';
import { QueueStore } from './store';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** Loading the core takes a moment; what the page sends meanwhile waits behind it, in order. */
let store: Promise<QueueStore> | undefined;

/** Starts the store on `init`; every other message waits for it, then goes to it. */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	if (data.type === 'init') {
		store = init({ module_or_path: data.wasm }).then(
			() =>
				new QueueStore({
					api: data.api,
					catalog: new CatalogRouter(),
					user: data.user,
					changed: (summary) => post({ type: 'state', summary }),
					refused: (refusal) => post({ type: 'refused', refusal })
				})
		);
	} else {
		store?.then((ready) => {
			if (data.type === 'message') ready.message(data.message);
			else if (data.type === 'edit') ready.edit(data.edit, data.length, data.offline);
			else if (data.type === 'start')
				post({ type: 'started', id: data.id, ok: ready.start(data.request) });
			else if (data.type === 'hidden') ready.hidden(data.libraryId, data.trackId, data.hidden);
			else if (data.type === 'restore')
				void ready
					.restore(data.queueId)
					.then((result) => post({ type: 'restored', id: data.id, result }));
			else post({ type: 'rows', id: data.id, window: ready.rows(data) });
		});
	}
};
