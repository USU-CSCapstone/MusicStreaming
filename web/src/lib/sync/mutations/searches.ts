// Recent searches (`requirements/search.md` §6): recording one, removing one and clearing them all.
// All of them, every library's, are one scope, so they reach the server in the order they were made,
// and the device makes each change at once through the server's own code. A search is recorded under an ID the
// client chose (`mintId`), so removing it later names the same search on the device and the server,
// whether or not it has been sent.

import {
	ApiError,
	clearRecentSearches,
	deleteRecentSearch,
	recordRecentSearch
} from '$lib/api/client';
import type { Selected } from '$lib/recent';
import type { Definition } from '../mutation';

/** A settled search, recorded under `id`, a positive 63-bit integer in decimal. */
export type RecordSearchArgs = {
	libraryId: string;
	id: string;
	query: string;
	selected?: Selected;
};

/** One recent search removed. */
export type DeleteSearchArgs = { libraryId: string; id: string; query: string };

/** Every recent search of a library cleared. */
export type ClearSearchesArgs = { libraryId: string };

/** The scope of the account's recent searches, every library's. */
export const searchScope = () => 'searches';

/**
 * A search recorded as the newest. One the server refuses, as one whose result has left the
 * library, is only a convenience lost, so nothing is said.
 */
export const recordSearch: Definition<RecordSearchArgs> = {
	scope: searchScope,
	send: ({ libraryId, id, query, selected }, { fetch, key }) =>
		recordRecentSearch(fetch, libraryId, { id, query, ...(selected && { selected }) }, key),
	apply: ({ libraryId, id, query, selected }) => ({
		library: libraryId,
		calls: [{ name: 'recordSearch', args: { id, query, selected } }]
	}),
	what: ({ query }) => `keep “${query}” in recent searches`,
	silent: true
};

/** A recent search removed. One the server no longer has is as removed as was wanted. */
export const deleteSearch: Definition<DeleteSearchArgs> = {
	scope: searchScope,
	async send({ libraryId, id }, { fetch, key }) {
		try {
			await deleteRecentSearch(fetch, libraryId, id, key);
		} catch (error) {
			if (!(error instanceof ApiError && error.status === 404)) throw error;
		}
	},
	apply: ({ libraryId, id }) => ({
		library: libraryId,
		calls: [{ name: 'deleteSearch', args: { id } }]
	}),
	what: ({ query }) => `remove “${query}” from recent searches`
};

/** Every recent search of a library cleared, those recorded before it and not yet sent too. */
export const clearSearches: Definition<ClearSearchesArgs> = {
	scope: searchScope,
	send: ({ libraryId }, { fetch, key }) => clearRecentSearches(fetch, libraryId, key),
	apply: ({ libraryId }) => ({ library: libraryId, calls: [{ name: 'clearSearches', args: {} }] }),
	what: () => 'clear recent searches'
};
