// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
import type { Library } from '$lib/api/types';

declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		interface PageData {
			/** The library being browsed; null when the account can reach none yet. */
			library: Library | null;
		}
		// interface PageState {}
		// interface Platform {}
	}
}

export {};
