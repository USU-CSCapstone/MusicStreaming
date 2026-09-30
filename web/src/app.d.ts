// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
import type { Library, User } from '$lib/api/types';

declare global {
	namespace App {
		// interface Error {}
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
