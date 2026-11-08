// The account and library list the layout needs, kept in this browser so the app starts with no
// network. Only that: the catalog itself lives in the worker's files. Cleared by the catalog's
// sign-out, which the next person to sign in must never see past (`requirements/general.md` §3.6).

import type { Library, ServerInfo, User } from '$lib/api/types';

export type LastKnown = { server?: ServerInfo; me?: User; libraries?: { items: Library[] } };

const KEY = 'jewelcase-last-known';

/** What was kept; nothing, where the browser has no storage to read. */
export function lastKnown(): LastKnown {
	try {
		return JSON.parse(localStorage.getItem(KEY) ?? '{}');
	} catch {
		return {};
	}
}

/** Keeps `part` beside what is kept already; storage that refuses it just keeps nothing. */
export function remember(part: LastKnown) {
	try {
		localStorage.setItem(KEY, JSON.stringify({ ...lastKnown(), ...part }));
	} catch {
		// Private windows and full disks: the app then needs the network to start.
	}
}

/** Deletes everything kept. */
export function forget() {
	try {
		localStorage.removeItem(KEY);
	} catch {
		// Nothing was kept.
	}
}
