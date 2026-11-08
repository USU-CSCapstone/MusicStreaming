// The catalog's web worker: a thin shell around `CatalogSync`, so the device's copy of the
// libraries is kept current off the page's thread (`design/general.md` §7.1). `client.svelte.ts`
// is the page's side; `protocol.ts` is what they say.

import init from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import { CatalogSync, describe } from './sync';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** What the page sent, one message after another: starting and signing out take a while. */
let work: Promise<CatalogSync | undefined> = Promise.resolve(undefined);

/** Loads the module on `init`; every other message waits for it, then goes to the sync. */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	if (data.type === 'init') {
		work = init({ module_or_path: data.wasm }).then(
			() =>
				new CatalogSync({
					api: data.api,
					user: data.user,
					report: (r) => post({ type: 'error', ...r })
				}),
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
			} else if (data.type === 'busy') {
				post({ type: 'answer', id: data.id, result: sync.busy() ? '1' : '' });
			} else if (data.type === 'query') {
				post({
					type: 'answer',
					id: data.id,
					result: await sync.query(data.library, data.name, data.params, data.offline)
				});
			} else {
				await sync.signOut();
				post({ type: 'answer', id: data.id, result: '' });
			}
		} catch (error) {
			if ('id' in data) post({ type: 'answer', id: data.id, error: describe(error).message });
			else post({ type: 'error', ...describe(error) });
		}
		return sync;
	});
};
