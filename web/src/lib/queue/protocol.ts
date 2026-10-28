// What the queue worker and the page say to each other (`design/general.md` §7.1). The worker
// holds the queue; the page sends it what happened and asks it for the windows it shows.

import type { PlayContext, QueueQuery, RealtimeServerMessage, SentEdit } from '$lib/api/types';

/** The parts of a queue a window can be read from. */
export type List = 'manual' | 'upcoming' | 'history';

/** What a device needs to start a queue; the store chooses the IDs. */
export type StartArgs = {
	/** What the queue started from, kept as sent. */
	context: PlayContext;
	libraryId: string;
	/** Start shuffled, with the order this seed gives. */
	shuffleSeed?: number;
} & (
	| {
			/** The tracks to play, in the order shown, when the device holds them all. */
			tracks: string[];
			/** The index of the track that plays first. */
			start: number;
	  }
	| {
			/** The list the view pages through, for the server to read to its end. */
			query: QueueQuery;
			/** The track that plays first; until the server answers, it is the whole queue. */
			startTrackId: string;
	  }
);

/** How the queue repeats: not at all, all of it, or the track playing. */
export type Repeat = 'off' | 'all' | 'one';

/** A question the page asks the queue; answered from the state it has now. */
export type Question =
	| { what: 'window'; list: List; offset: number; limit: number }
	| { what: 'track'; item: string };

/** The queue as the page shows it, sent whenever it changes. */
export type Summary = {
	/** Null until there is a queue. */
	queueId: string | null;
	/** What the queue started from, as the server keeps it, for a play to record; null with no queue. */
	context: PlayContext | null;
	/** The server's version of the queue, as far as this device has applied it. */
	version: number;
	/** Counts up with every change, so a window read earlier is known to be stale. */
	revision: number;
	/** The item playing now. */
	current: string | null;
	manual: number;
	upcoming: number;
	history: number;
	repeat: Repeat;
	/** The context's Up Next is in a shuffled order. */
	shuffled: boolean;
	/** A start from a list is under way: the queue is just its first track until the server's arrives. */
	settling: boolean;
	/** The server has not yet taken the latest start, so it would refuse a report naming its items. */
	starting: boolean;
};

/** An edit or start the queue could not keep, so the page can say so (`requirements/queue.md` §7). */
export type Refusal = {
	/** The server's problem code, such as `anchor_gone`, or `invalid` for what the core refused. */
	code: string;
	message: string;
	/** The edit that was reverted; absent for a start. */
	edit?: SentEdit;
};

/** The track an item plays. */
export type ItemTrack = { libraryId: string; trackId: string };

/** What caches of a queue's tracks are keyed by: track IDs are only unique within a library. */
export const trackKey = ({ libraryId, trackId }: ItemTrack) => `${libraryId}/${trackId}`;

/** The page to the worker. */
export type ToWorker =
	/** First: where the API is. */
	| { type: 'init'; api: string }
	| { type: 'message'; message: RealtimeServerMessage }
	| { type: 'start'; args: StartArgs }
	| { type: 'edit'; edit: SentEdit }
	| { type: 'ask'; id: number; question: Question };

/** The worker to the page. */
export type FromWorker =
	| { type: 'state'; summary: Summary }
	| { type: 'refused'; refusal: Refusal }
	| { type: 'answer'; id: number; value: string[] | ItemTrack | null }
	| { type: 'failed'; id: number; error: string };
