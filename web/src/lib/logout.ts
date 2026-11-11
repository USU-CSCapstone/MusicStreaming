// A logout the server may not have heard of. The session is an HttpOnly cookie, which only the
// server can clear, so a device that logs out with no server in reach wipes itself at once,
// leaves a marker (`last-known.ts`), and tells the server the first time it can.

import { isRedirect } from '@sveltejs/kit';
import { logout } from '$lib/api/client';
import { logoutPending, setLogoutPending } from '$lib/last-known';

/**
 * Tells the server this browser logged out, and takes the marker down once it knows: it answered,
 * or said the session was over already. A server that cannot be reached, or fails, leaves the
 * marker for the next try. Does nothing without a marker, so a login in between is not undone.
 * Resolves whether the marker is down.
 */
async function tell(fetcher: typeof fetch): Promise<boolean> {
	if (!logoutPending()) return true;
	try {
		await logout(fetcher);
	} catch (error) {
		if (!isRedirect(error)) return false;
	}
	setLogoutPending(false);
	return true;
}

/** [`tell`](#tell), and if that fails, again when the browser says it is online. */
export async function finishLogout(fetcher: typeof fetch) {
	if (!(await tell(fetcher)))
		addEventListener('online', () => void finishLogout(fetcher), { once: true });
}
