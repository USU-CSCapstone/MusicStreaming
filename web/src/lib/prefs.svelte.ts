// Appearance preferences. Per browser for now; they become account preferences
// that follow the person once the API has them (`requirements/users.md` §6.1).

export type CollectionView = 'grid' | 'list';

const VIEW_KEY = 'jewelcase.view';

function read(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function write(key: string, value: string) {
	try {
		localStorage.setItem(key, value);
	} catch {
		// Storage can be unavailable (private windows, blocked site data); the choice lasts the session.
	}
}

class Prefs {
	#view = $state<CollectionView>(read(VIEW_KEY) === 'list' ? 'list' : 'grid');

	get view(): CollectionView {
		return this.#view;
	}

	set view(v: CollectionView) {
		this.#view = v;
		write(VIEW_KEY, v);
	}
}

export const prefs = new Prefs();
