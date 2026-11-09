// The catalog's web worker: a thin shell around `CatalogSync`, so the device's copy of the
// libraries is kept current off the page's thread (`design/general.md` §7.1). `client.svelte.ts`
// is the page's side; `protocol.ts` is what they say.

import init from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import { gated } from '$lib/gated-fetch';
import { CatalogSync, describe } from './sync';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** What the page sent, one message after another: starting and signing out take a while. */
let work: Promise<CatalogSync | undefined> = Promise.resolve(undefined);

/** The sync, once the module has loaded: what the questions answered outside the queue ask. */
let current: CatalogSync | undefined;

/** The device is working offline: no request leaves, and nothing pulls. */
let workingOffline = false;

/** The questions about the sync's state, which are answered ahead of queued work. */
type State = { type: 'onDevice' | 'indexed'; library: string } | { type: 'busy' };

/** Whether `data` is a question about the sync's state. */
const isState = (data: ToWorker): data is ToWorker & State =>
	data.type === 'onDevice' || data.type === 'indexed' || data.type === 'busy';

/** What `sync` says to `data`, a question about its state. */
const stateOf = (sync: CatalogSync, data: State) =>
	data.type === 'busy'
		? sync.busy()
		: data.type === 'indexed'
			? sync.indexed(data.library)
			: sync.onDevice(data.library);

/**
 * Handles a message from the page: `offline` takes effect at once, `init` loads the module
 * and makes the sync, `onDevice`, `indexed` and `busy` are answered from the sync's state ahead
 * of queued work, and every other message waits, then goes to the sync in order. The answers
 * come between the worker's synchronous wasm calls, so they wait only on the call under way,
 * but one call can outlast an asker's wait: an apply chunk, an index build or a snapshot install
 * each measured seconds on a phone at 4× throttling.
 */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	// At once, ahead of the work already queued: a switch must stop what comes next.
	if (data.type === 'offline') {
		workingOffline = data.value;
		return;
	}
	if (current && isState(data)) {
		post({ type: 'answer', id: data.id, result: stateOf(current, data) ? '1' : '' });
		return;
	}
	if (data.type === 'init') {
		work = init({ module_or_path: data.wasm }).then(
			() =>
				(current = new CatalogSync({
					api: data.api,
					user: data.user,
					fetch: gated(() => workingOffline),
					online: () => navigator.onLine && !workingOffline,
					report: (r) => post({ type: 'error', ...r }),
					changed: (libraryId) => post({ type: 'changed', libraryId })
				})),
			(error) => {
				post({ type: 'error', ...describe(error) });
				return undefined;
			}
		);
		return;
	}
	work = work.then(async (sync) => {
		// Without a module, requests fail and `started` is never sent, so the page gives the lock up.
		if (!sync) {
			if ('id' in data) post({ type: 'answer', id: data.id, error: 'the catalog is not running' });
			else if (data.type === 'pause') post({ type: 'paused' });
			return sync;
		}
		try {
			if (data.type === 'start') {
				await sync.start().catch((error) => post({ type: 'error', ...describe(error) }));
				post({ type: 'started' });
			} else if (data.type === 'message') sync.message(data.message);
			else if (data.type === 'pause') {
				await sync.pause();
				post({ type: 'paused' });
			} else if (data.type === 'prepareIndex') {
				sync.prepareIndex(data.library);
				post({ type: 'answer', id: data.id, result: '' });
			} else if (data.type === 'query') {
				post({
					type: 'answer',
					id: data.id,
					result: await sync.query(data.library, data.name, data.params, data.offline)
				});
			} else if (data.type === 'signOut') {
				await sync.signOut();
				post({ type: 'answer', id: data.id, result: '' });
			} else {
				// A newer build's request, from a tab of that version: its asker reads the server.
				throw new Error('unknown request');
			}
		} catch (error) {
			if ('id' in data) post({ type: 'answer', id: data.id, error: describe(error).message });
			else post({ type: 'error', ...describe(error) });
		}
		return sync;
	});
};
