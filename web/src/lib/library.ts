import { error } from '@sveltejs/kit';

/** Thrown when there is no library to browse; the root error page explains it. */
export const NO_LIBRARY = 'no-library';

/** The library a page loads from, or the "no music yet" state when there is none. */
export async function requireLibrary(parent: () => Promise<App.PageData>): Promise<string> {
	const { library } = await parent();
	if (!library) error(404, NO_LIBRARY);
	return library.id;
}
