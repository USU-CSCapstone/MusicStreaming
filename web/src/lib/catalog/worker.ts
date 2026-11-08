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
				})
		);
		return;
	}
	work = work.then(async (sync) => {
		try {
			if (data.type === 'start') await sync?.start();
			else if (data.type === 'message') sync?.message(data.message);
			else {
				await sync?.signOut();
				post({ type: 'signedOut' });
			}
		} catch (error) {
			post({ type: 'error', ...describe(error) });
		}
		return sync;
	});
};
