// Appearance preferences. Per browser for now; they become account preferences
// that follow the person once the API has them (`requirements/users.md` §6.1).

export type CollectionView = 'grid' | 'list';

const VIEW_KEY = 'jewelcase.view';
const HIDDEN_TAG_COLUMNS_KEY = 'jewelcase.hiddenTagColumns';

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

/** The tag types stored as hidden: a list of keys, or none for anything unreadable. */
function readHidden(): string[] {
	try {
		const stored: unknown = JSON.parse(read(HIDDEN_TAG_COLUMNS_KEY) ?? '[]');
		return Array.isArray(stored) ? stored.filter((key) => typeof key === 'string') : [];
	} catch {
		return [];
	}
}

class Prefs {
	#view = $state<CollectionView>(read(VIEW_KEY) === 'list' ? 'list' : 'grid');
	/**
	 * The tag types not shown as columns in song lists. Kept as the hidden ones, so a type the
	 * library gains, a plugin's among them, shows until it is hidden.
	 */
	#hiddenTagColumns = $state<string[]>(readHidden());

	get view(): CollectionView {
		return this.#view;
	}

	set view(v: CollectionView) {
		this.#view = v;
		write(VIEW_KEY, v);
	}

	/** Whether song lists show tag type `key` as a column. */
	showsTagColumn(key: string): boolean {
		return !this.#hiddenTagColumns.includes(key);
	}

	/** Shows tag type `key` as a column in song lists, or hides it. */
	showTagColumn(key: string, shown: boolean) {
		const others = this.#hiddenTagColumns.filter((hidden) => hidden !== key);
		this.#hiddenTagColumns = shown ? others : [...others, key];
		write(HIDDEN_TAG_COLUMNS_KEY, JSON.stringify(this.#hiddenTagColumns));
	}
}

export const prefs = new Prefs();
