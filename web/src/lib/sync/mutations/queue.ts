// The account's queue (`design/queue.md` §7): starts, edits, restores and deletes of kept queues
// are mutations of the one `queue` scope, so the server hears them strictly in the order they
// were made, as it would have online. An edit names the queue it was made on, and the server
// applies it only while that queue is active, so the edits made on a queue before another start
// reach it first, and the start then parks it in Kept with them (`requirements/queue.md` §6). The
// queue worker makes each change on the device at once with the queue's core, and learns what the
// server answered from the sender (`queue/client.svelte.ts`); the catalog is not touched.

import {
	ApiError,
	deleteKeptQueue as removeKeptQueue,
	editQueue,
	restoreKeptQueue,
	startQueue
} from '$lib/api/client';
import type { EditQueue, StartQueue } from '$lib/api/types';
import type { Definition } from '../mutation';

/** The one scope of the account's queue. */
export const QUEUE_SCOPE = 'queue';

/**
 * A start: the request as the server takes it, and what the device needs to show the new queue
 * again from the log before the server answers, which is never sent.
 */
export type StartArgs = {
	body: StartQueue;
	/** How long the view said the source was. */
	total?: number;
	/** The rows the view had loaded of a recipe's source. */
	loaded?: string[];
};

/** An edit, as sent, and the count of rows an `add` of a recipe shows until the server says. */
export type EditArgs = EditQueue & { length?: number };

/** A kept queue made the active one. */
export type RestoreArgs = { queueId: string };

/** A kept queue deleted. */
export type DeleteKeptArgs = { queueId: string };

/** The queue a start or a restore of `args` makes the active one; none for any other kind. */
const madeActive = (args: unknown, name: string): string | undefined =>
	name === 'startQueue'
		? (args as StartArgs).body.queueId
		: name === 'restoreQueue'
			? (args as RestoreArgs).queueId
			: undefined;

/** A queue started. Its answer is the server's list for the source. */
export const startQueueMutation: Definition<StartArgs> = {
	scope: () => QUEUE_SCOPE,
	send: ({ body }, { fetch, key }) => startQueue(fetch, body, key),
	what: () => 'start that queue',
	silent: true
};

/**
 * An edit of the queue it names. Made on a queue whose start or restore was refused, it goes with
 * it: the server would refuse it as for another queue. Its answer is the version it made.
 */
export const editQueueMutation: Definition<EditArgs> = {
	scope: () => QUEUE_SCOPE,
	send: ({ editId, queueId, edit }, { fetch, key }) =>
		editQueue(fetch, { editId, queueId, edit }, key),
	depends: (later, refused, name) => madeActive(refused, name) === later.queueId,
	what: () => 'change the queue',
	silent: true
};

/** A kept queue restored. Its answer is the queue, and the run of its `ETag`. */
export const restoreQueueMutation: Definition<RestoreArgs> = {
	scope: () => QUEUE_SCOPE,
	send: ({ queueId }, { fetch, key }) => restoreKeptQueue(fetch, queueId, key),
	what: () => 'restore that queue',
	silent: true
};

/** A kept queue deleted. One the server no longer has is as deleted as was wanted. */
export const deleteKeptQueueMutation: Definition<DeleteKeptArgs> = {
	scope: () => QUEUE_SCOPE,
	async send({ queueId }, { fetch, key }) {
		try {
			await removeKeptQueue(fetch, queueId, key);
		} catch (error) {
			if (!(error instanceof ApiError && error.status === 404)) throw error;
		}
	},
	what: () => 'delete that kept queue',
	refused: (_, code) =>
		code === 'queue_active' ? 'That queue is the one playing now, so it wasn’t deleted.' : undefined
};
