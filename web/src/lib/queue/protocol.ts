// What the queue worker and the page say to each other (`design/web.md`, Queue worker). The
// worker holds the queue; the page sends it what happened and shows what it reports.

import type { PlayContext, RealtimeServerMessage, SentEdit } from '$lib/api/types';

/** How the queue repeats: not at all, all of it, or the item playing. */
export type Repeat = 'off' | 'all' | 'one';

/** The queue as the page shows it, sent whenever it changes. It holds no tracks. */
export type Summary = {
	/** Null until there is a queue. */
	queueId: string | null;
	/** What the queue started from, as the server keeps it; null with no queue. */
	context: PlayContext | null;
	/** The server's version of the queue, as far as this device has applied it. */
	version: number;
	/** Counts up with every change, so something read earlier is known to be stale. */
	revision: number;
	/** The item playing now. */
	current: string | null;
	/** The rows of the Manual Queue, which is the top of Up Next, and of all of Up Next. */
	manual: number;
	upNext: number;
	history: number;
	repeat: Repeat;
	/** The context's Up Next is in a shuffled order. */
	shuffled: boolean;
};

/** An edit the queue could not keep, so the page can say so (`requirements/queue.md` §7). */
export type Refusal = {
	/**
	 * Why: the server's problem code such as `anchor_gone` or `play_gone`, the core's code for an
	 * edit it refuses, `superseded` when another queue replaced this one, or `lost` when the
	 * server answered the edit but no longer has it, as after a restart.
	 */
	code: string;
	message: string;
	edit: SentEdit;
};

/** The page to the worker. */
export type ToWorker =
	/** First: where the API is. */
	| { type: 'init'; api: string }
	| { type: 'message'; message: RealtimeServerMessage }
	| { type: 'edit'; edit: SentEdit };

/** The worker to the page. */
export type FromWorker =
	| { type: 'state'; summary: Summary }
	| { type: 'refused'; refusal: Refusal };
