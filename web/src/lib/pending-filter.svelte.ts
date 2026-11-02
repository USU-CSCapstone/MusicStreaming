// The filter chosen on a page whose load has not finished, so the filter's controls show a change
// at once and the next change builds on it rather than on the filter last loaded
// (`requirements/general.md` §3.4).

import type { TagFilter } from './tag-match';

/** A filter chosen on the page at `path`. */
export type Pending = { path: string; filter: TagFilter };

// Raw, so the filter kept is the one given and `settle` can tell it apart from a later one.
let pending = $state.raw<Pending | null>(null);

/** The filter chosen and not yet loaded, if any, with the page it was chosen on. */
export function pendingFilter(): Pending | null {
	return pending;
}

/** Notes `filter` as chosen on the page at `path`, until its load settles. */
export function choose(path: string, filter: TagFilter) {
	pending = { path, filter };
}

/** Forgets `filter` once its load has finished or failed, unless another was chosen since. */
export function settle(filter: TagFilter) {
	if (pending?.filter === filter) pending = null;
}
