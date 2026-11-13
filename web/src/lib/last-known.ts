// What this browser keeps about who is signed in, so the app starts with no network: the server's
// public details, the account that owns the device and, for that account alone, its user and
// library list, and which libraries its files hold. Only that: the catalog itself lives in the
// worker's files. Cleared by the catalog's sign-out, which the next person to sign in must never
// see past (`requirements/general.md` §3.6).

import type { Library, ServerInfo, User } from '$lib/api/types';

/** What the browser keeps: the server's details, and the owner's user and library list. */
export type LastKnown = { server?: ServerInfo; me?: User; libraries?: { items: Library[] } };

/**
 * What each kept key is stored under: the server's details, the owner's ID, and an account's own
 * part.
 */
const SERVER = 'jewelcase-server';
/** The `localStorage` key of the account that owns the device. */
const OWNER = 'jewelcase-owner';
/** The start of the key of an account's user and library list; the user's ID follows. */
const USER = 'jewelcase-user:';
/**
 * The key of the libraries an account has on this device, which an app launched offline reads
 * before its owner has said.
 */
const DEVICE = 'jewelcase-device:';
/** The marker of a logout the server has not been told of. */
export const LOGOUT_PENDING = 'jewelcase-logout-pending';

/**
 * The JSON kept under `key`; nothing, where there is none or the browser has no storage to read.
 */
function read(key: string): Record<string, unknown> | undefined {
	try {
		return JSON.parse(localStorage.getItem(key) ?? 'null') ?? undefined;
	} catch {
		return undefined;
	}
}

/** Keeps `value` under `key`; storage that refuses it just keeps nothing. */
function write(key: string, value: unknown) {
	try {
		localStorage.setItem(key, JSON.stringify(value));
	} catch {
		// Private windows and full disks: the app then needs the network to start.
	}
}

/** The ID of the account whose data this device holds, or null. */
export function owner(): string | null {
	try {
		return localStorage.getItem(OWNER);
	} catch {
		return null;
	}
}

/** Says the device now belongs to `user`. */
export function claim(user: string) {
	try {
		localStorage.setItem(OWNER, user);
	} catch {
		// Nothing is kept, so there is nothing to confuse.
	}
}

/** What was kept: the server's details, and the owner's user and libraries. */
export function lastKnown(): LastKnown {
	const user = owner();
	return { server: read(SERVER), ...(user ? read(USER + user) : {}) } as LastKnown;
}

/**
 * Keeps `part` beside what is kept already: the server's details for anyone, and a user and
 * libraries under the account they belong to, the user's own or else the device's owner.
 */
export function remember(part: LastKnown) {
	const { server, ...own } = part;
	const user = part.me?.id ?? owner();
	if (server) write(SERVER, server);
	if (user && Object.keys(own).length) write(USER + user, { ...read(USER + user), ...own });
}

/** Keeps the IDs of the libraries `user` has on this device, as its owner last said. */
export function rememberDevice(user: string, libraries: string[]) {
	write(DEVICE + user, libraries);
}

/** The IDs of the libraries `user` had on this device when its owner last said, or none. */
export function deviceLibraries(user: string): Set<string> {
	const kept: unknown = read(DEVICE + user);
	return new Set(Array.isArray(kept) ? kept.map(String) : []);
}

/**
 * Deletes everything kept of any account and who owns the device; the server's details are not an
 * account's.
 */
export function forget() {
	try {
		for (const key of Object.keys(localStorage)) {
			if (key === OWNER || key.startsWith(USER) || key.startsWith(DEVICE))
				localStorage.removeItem(key);
		}
	} catch {
		// Nothing was kept.
	}
}

/** Whether a logout is waiting to be told to the server. */
export function logoutPending(): boolean {
	try {
		return localStorage.getItem(LOGOUT_PENDING) === '1';
	} catch {
		return false;
	}
}

/** Sets or clears the marker of a logout the server has not heard of. */
export function setLogoutPending(on: boolean) {
	try {
		if (on) localStorage.setItem(LOGOUT_PENDING, '1');
		else localStorage.removeItem(LOGOUT_PENDING);
	} catch {
		// Nothing can be kept; the logout is still sent now, if the server is there.
	}
}
