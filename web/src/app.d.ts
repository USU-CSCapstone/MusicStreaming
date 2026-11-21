// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
import type { Library, User } from '$lib/api/types';

declare global {
	namespace App {
		interface Error {
			message: string;
			/** Why the device cannot show the page, for the states the error page explains. */
			code?: string;
			/** The server's reason for a `forbidden`, when it gave one. */
			detail?: string;
		}
		// interface Locals {}
		interface PageData {
			/** The library being browsed; null when the account can reach none yet. */
			library: Library | null;
			/** The logged-in account; null on the setup and login pages. */
			me: User | null;
		}
		// interface PageState {}
		// interface Platform {}
	}
}

export {};
