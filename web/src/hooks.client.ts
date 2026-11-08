import type { HandleClientError } from '@sveltejs/kit';
import { ApiError } from '$lib/api/root';

/** Codes of the errors the error page explains: a library missing from the device, or a page that needs the server. */
const SHOWN = new Set(['not_on_device', 'unavailable_offline']);

/** Passes the code of an error the error page explains on to it; any other error is logged and shows SvelteKit's message. */
export const handleError: HandleClientError = ({ error, message }) => {
	if (error instanceof ApiError && error.code && SHOWN.has(error.code))
		return { message: error.message, code: error.code };
	console.error(error);
	return { message };
};
