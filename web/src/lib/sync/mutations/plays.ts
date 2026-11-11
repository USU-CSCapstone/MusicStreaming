// Plays (`requirements/analytics.md` §1): each report of a play is a mutation of the one `plays`
// scope, which replaces the earlier report of its play and is sent with others, up to a batch at a
// time. The server keeps the largest listen time a play's reports gave, so their order is no
// matter.

import { recordPlays } from '$lib/api/client';
import type { PlayReport } from '$lib/api/types';
import type { Definition } from '../mutation';

/** The most plays one request carries (`api/openapi.yaml`, `recordPlays`). */
export const BATCH = 500;

/** A report of one play. */
export const play: Definition<PlayReport> = {
	scope: () => 'plays',
	send: (report, { fetch }) => recordPlays(fetch, [report]),
	batch: { size: BATCH, send: (reports, fetch) => recordPlays(fetch, reports) },
	merge: (pending, next) => (pending.playId === next.playId ? next : undefined),
	what: () => 'record a play'
};
