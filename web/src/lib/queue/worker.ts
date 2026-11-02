// The queue's web worker: a thin shell around `QueueStore`, so loading and editing a queue of
// any size is off the page's thread (`design/web.md`, Queue worker). `client.svelte.ts` is the
// page's side; `protocol.ts` is what they say.

import init from '$lib/core-wasm/jewelcase_core_wasm.js';
import { QueueStore } from './store';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** Loading the core takes a moment; what the page sends meanwhile waits behind it, in order. */
let store: Promise<QueueStore> | undefined;

/** Starts the store on `init`; every other message waits for it, then goes to it. */
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
		store?.then((ready) =>
			data.type === 'message' ? ready.message(data.message) : ready.edit(data.edit)
		);
	}
};
