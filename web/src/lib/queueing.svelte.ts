// Adding to the account's queue from anywhere (`requirements/conventions.md` §2, `queue.md` §1,
// §3): Queue Next and Queue Last from a menu, or a drop at a point of Up Next. What is added
// is an `Adding`, as for a playlist: songs, or a whole album, artist or playlist.

import type { Adding } from './adding.svelte';
import { ApiError } from './api/client';
import type { QueueAnchor } from './api/types';
import { panels } from './panels.svelte';
import { queue } from './queue/client.svelte';
import { toast } from './toast.svelte';

/** What Queue Next, Queue Last and a drop at a point say they did. */
const SAID: Record<QueueAnchor['type'], string> = {
	manualStart: 'queued next',
	manualEnd: 'queued last',
	contextEnd: 'added to Up Next',
	before: 'added to Up Next'
};

/**
 * What is waiting to be put at a point of Up Next that the person chooses in the queue panel:
 * the keyboard and touch way to drop (`requirements/queue.md` §3), where a mouse drags.
 */
export const placing = $state<{ what: Adding | null; libraryId: string }>({
	what: null,
	libraryId: ''
});

/** Opens the queue panel to choose where `what`, which is in library `libraryId`, goes. */
export function startPlacing(what: Adding, libraryId: string) {
	if (!hasQueue()) return;
	placing.what = what;
	placing.libraryId = libraryId;
	panels.queue = true;
}

/** Whether there is a queue to add to; says so if not, since there is nothing to queue into. */
function hasQueue(): boolean {
	if (queue.queueId) return true;
	toast('Start something playing first, then queue more.');
	return false;
}

/**
 * Adds `what`, which is in library `libraryId`, to the queue at `before`, saying so at once
 * (`requirements/general.md` §3.4). What has a recipe goes as that, for the server to resolve:
 * it shows at once as as many rows as `what` counts, whose songs show once the server has
 * resolved them. Anything else goes as its tracks, which show at once.
 * A refusal is explained by the queue panel. Returns whether an edit was sent.
 */
export async function addToQueue(
	what: Adding,
	libraryId: string,
	before: QueueAnchor
): Promise<boolean> {
	if (!hasQueue()) return false;
	const key = crypto.randomUUID();
	try {
		if (what.recipe) {
			// A snapshot, since `what` may be a proxy of `placing`, which cannot be posted to the worker.
			const recipe = $state.snapshot(what.recipe);
			queue.edit({ type: 'add', source: { key, libraryId, recipe }, before }, what.count);
		} else {
			const tracks = (await what.tracks()).map((track) => track.id);
			if (tracks.length === 0) {
				toast(`There are no songs in ${what.label} to queue.`);
				return false;
			}
			queue.edit({ type: 'add', source: { key, libraryId, tracks }, before });
		}
		toast(`${what.label} ${SAID[before.type]}`);
		return true;
	} catch (e) {
		toast(
			`Couldn’t add ${what.label} to the queue. ${e instanceof ApiError ? e.message : ''}`.trim()
		);
		return false;
	}
}
