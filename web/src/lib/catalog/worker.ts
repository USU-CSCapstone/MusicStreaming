// The catalog's web worker: a thin shell around `CatalogSync`, so the device's copy of the
// libraries is kept current off the page's thread (`design/general.md` §7.1). `client.svelte.ts`
// is the page's side; `protocol.ts` is what they say.

import init from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import { gated } from '$lib/gated-fetch';
import { dropLibraryImages } from '$lib/image-cache';
import { dropLibraryLists } from '$lib/queue/lists';
import type { Mutation } from '$lib/sync/mutation';
import { IdbPort } from '$lib/sync/store';
import { CatalogSync, describe } from './sync';
import type { FromWorker, ToWorker } from './protocol';

/** Sends a message to the page. */
const post = (message: FromWorker) => postMessage(message);

/** What the page sent, one message after another: starting and signing out take a while. */
let work: Promise<CatalogSync | undefined> = Promise.resolve(undefined);

/** The sync, once the module has loaded: what `busy` is answered from, outside the queue. */
let current: CatalogSync | undefined;

/** The device is working offline: no request leaves, and nothing pulls. */
let workingOffline = false;

/** The sync is pausing or paused, so a read that fails now was cut short by it. */
let pausing = false;

/**
 * Handles a message from the page: `offline` takes effect at once, `init` loads the module
 * and makes the sync, `busy` is answered from the sync's state ahead of queued work, and every
 * other message waits, then goes to the sync in order. A read goes to the sync in its turn but is
 * answered whenever it settles, so one waiting for an index build or an apply holds up neither
 * other reads nor a pause; one the pause cuts short is left unanswered, and the asking tab sends
 * it to the next owner. The sync tells the page what each library stands at whenever that
 * changes, so no one asks. The answer to `busy` comes between the worker's synchronous wasm
 * calls, so it waits only on the call under way, but one call can outlast an asker's wait: an
 * apply chunk, an index build or a snapshot install each measured seconds on a phone at 4×
 * throttling.
 */
onmessage = ({ data }: MessageEvent<ToWorker>) => {
	// At once, ahead of the work already queued: a switch must stop what comes next.
	if (data.type === 'offline') {
		workingOffline = data.value;
		return;
	}
	if (current && data.type === 'busy') {
		post({ type: 'answer', id: data.id, result: current.busy() ? '1' : '' });
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
					changed: (libraryId, kinds) => post({ type: 'changed', libraryId, kinds }),
					state: (libraries) => post({ type: 'state', libraries }),
					heavy: (on) => post({ type: 'heavy', on }),
					lost: async (libraryId) => {
						await dropLibraryImages(data.user, libraryId, data.api);
						await dropLibraryLists(data.user, libraryId);
					},
					pending: (libraryId) => pending(data.user, libraryId)
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
				const found = await sync.start().catch((error) => {
					post({ type: 'error', ...describe(error) });
					return undefined;
				});
				post({ type: 'started', found });
			} else if (data.type === 'message') sync.message(data.message);
			else if (data.type === 'pause') {
				pausing = true;
				await sync.pause();
				post({ type: 'paused' });
			} else if (data.type === 'query') {
				const { id } = data;
				sync.query(data.library, data.name, data.params, data.offline).then(
					(result) => post({ type: 'answer', id, result }),
					(error) => {
						if (!pausing) post({ type: 'answer', id, error: describe(error).message });
					}
				);
			} else if (data.type === 'mutate' || data.type === 'rebase' || data.type === 'confirmed') {
				const { id } = data;
				const done =
					data.type === 'mutate'
						? sync.mutate(data.library, data.calls, data.answered)
						: data.type === 'rebase'
							? sync.rebase(data.library)
							: sync.confirmed(data.library, data.playlistId, data.version);
				done.then(
					() => post({ type: 'answer', id, result: '' }),
					(error) => {
						if (!pausing) post({ type: 'answer', id, error: describe(error).message });
					}
				);
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

/** `user`'s log, which the worker reads the device's own writes from; opened on first use. */
let log: IdbPort | undefined;

/**
 * The writes of the mutations `user`'s log holds for library `library`, in the order they were
 * made. A log that cannot be read gives none, so the device shows the server's rows alone.
 */
async function pending(user: string, library: string) {
	log ??= new IdbPort(user);
	try {
		const mutations = await log.all<Mutation>('mutations');
		return mutations
			.filter((mutation) => mutation.local?.library === library)
			.sort((a, b) => a.seq - b.seq)
			.flatMap((mutation) => mutation.local!.calls);
	} catch (error) {
		console.error(error);
		return [];
	}
}
