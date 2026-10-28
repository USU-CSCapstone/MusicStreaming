// Adding to the account's queue from anywhere (`requirements/conventions.md` §2, `queue.md` §1,
// §3): Queue Next and Queue Last from a menu, or a drop at a point of Up Next. What is added
// is an `Adding`, as for a playlist: songs, or a whole album, artist or playlist.

import type { Adding } from './adding.svelte';
import { ApiError } from './api/client';
import type { QueueAnchor } from './api/types';
import { queue } from './queue/client.svelte';
import { toast } from './toast.svelte';

/** What Queue Next, Queue Last and a drop on a gap say they did. */
const SAID: Record<QueueAnchor['type'], string> = {
	manualStart: 'queued next',
	manualEnd: 'queued last',
	contextEnd: 'added to Up Next',
	before: 'added to Up Next'
};

/**
 * Adds `what`, which is in library `libraryId`, to the queue at `before`, saying so at once
 * (`requirements/general.md` §3.4). Songs the device holds go as tracks and show at once; an
 * album or artist it does not hold goes as a query for the server to read, and shows when the
 * server answers; a playlist's songs are fetched first. A refusal is explained by the queue
 * client's toast.
 */
export async function addToQueue(what: Adding, libraryId: string, before: QueueAnchor) {
	if (!queue.queueId) {
		toast('Start something playing first, then queue more.');
		return;
	}
	const key = crypto.randomUUID();
	try {
		if (what.query) {
			queue.edit({ type: 'add', source: { key, libraryId, query: what.query }, before });
		} else {
			const tracks = (await what.tracks()).map((track) => track.id);
			if (tracks.length === 0) return;
			queue.edit({ type: 'add', source: { key, libraryId, tracks }, before });
		}
		toast(`${what.label} ${SAID[before.type]}`);
	} catch (e) {
		toast(
			`Couldn’t add ${what.label} to the queue. ${e instanceof ApiError ? e.message : ''}`.trim()
		);
	}
}
