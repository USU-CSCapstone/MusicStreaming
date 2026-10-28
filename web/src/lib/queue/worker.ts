// The queue's web worker: a thin shell around `QueueStore`, so the work of loading and editing
// a queue of any size is off the page's thread (`design/general.md` §7.1). `client.svelte.ts` is
// the page's side; `protocol.ts` is what they say.

import init from '$lib/core-wasm/jewelcase_core_wasm.js';
import { QueueStore } from './store';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** Loading the core takes a moment; what the page sends meanwhile waits behind it, in order. */
let store: Promise<QueueStore> | undefined;

/** Hands what the page sent to the store, and answers what it asked. */
function handle(store: QueueStore, message: ToWorker) {
	if (message.type === 'message') store.message(message.message);
	else if (message.type === 'start') store.start(message.args);
	else if (message.type === 'edit') store.edit(message.edit);
	else if (message.type === 'ask') {
		try {
			post({ type: 'answer', id: message.id, value: store.ask(message.question) });
		} catch (error) {
			post({ type: 'failed', id: message.id, error: String(error) });
		}
	}
}

/** Starts the store on `init`; every other message waits for it, then goes to `handle`. */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	if (data.type === 'init') {
		store = init().then(
			() =>
				new QueueStore({
					api: data.api,
					changed: (summary) => post({ type: 'state', summary }),
					refused: (refusal) => post({ type: 'refused', refusal })
				})
		);
	} else {
		store?.then((ready) => handle(ready, data));
	}
};
