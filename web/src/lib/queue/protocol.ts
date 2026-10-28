// What the queue worker and the page say to each other (`design/general.md` §7.1). The worker
// holds the queue; the page sends it what happened and asks it for the windows it shows.

import type { QueueEdit, RealtimeServerMessage, StartQueue } from '$lib/api/types';

/** The parts of a queue a window can be read from. */
export type List = 'manual' | 'upcoming' | 'history';

/** What a device needs to start a queue; the store chooses the IDs. */
export type StartArgs = {
	/** What the queue started from, kept as sent. */
	context: StartQueue['context'];
	libraryId: string;
	/** The tracks to play, in the order shown. */
	tracks: string[];
	/** The index of the track that plays first. */
	start: number;
	/** Start shuffled, with the order this seed gives. */
	shuffleSeed?: number;
};

/** A question the page asks the queue; answered from the state it has now. */
export type Question =
	| { what: 'window'; list: List; offset: number; limit: number }
	| { what: 'track'; item: string };

/** The queue as the page shows it, sent whenever it changes. */
export type Summary = {
	/** Null until there is a queue. */
	queueId: string | null;
	/** The server's version of the queue, as far as this device has applied it. */
	version: number;
	/** Counts up with every change, so a window read earlier is known to be stale. */
	revision: number;
	/** The item playing now. */
	current: string | null;
	manual: number;
	upcoming: number;
	history: number;
};

/** An edit or start the queue could not keep, so the page can say so (`requirements/queue.md` §7). */
export type Refusal = {
	/** The server's problem code, such as `anchor_gone`, or `invalid` for what the core refused. */
	code: string;
	message: string;
	/** The edit that was reverted; absent for a start. */
	edit?: QueueEdit;
};

/** The track an item plays. */
export type ItemTrack = { libraryId: string; trackId: string };

/** The page to the worker. */
export type ToWorker =
	/** First: where the API is. */
	| { type: 'init'; api: string }
	| { type: 'message'; message: RealtimeServerMessage }
	| { type: 'start'; args: StartArgs }
	| { type: 'edit'; edit: QueueEdit }
	| { type: 'ask'; id: number; question: Question };

/** The worker to the page. */
export type FromWorker =
	| { type: 'state'; summary: Summary }
	| { type: 'refused'; refusal: Refusal }
	| { type: 'answer'; id: number; value: string[] | ItemTrack | null }
	| { type: 'failed'; id: number; error: string };
