// A list's sort lives in its URL, `?sort=artist&order=desc`, so a reload or Back returns to the
// list as it was sorted.

import { goto } from '$app/navigation';
import { page } from '$app/state';
import type { SortField, TrackSort } from './sorting';

/** The sort `params` hold, if it is one this list offers; otherwise unsorted. */
export function readSort(params: URLSearchParams, fields: readonly SortField[]): TrackSort {
	const field = params.get('sort');
	if (!fields.includes(field as SortField)) return null;
	return { field: field as SortField, order: params.get('order') === 'desc' ? 'desc' : 'asc' };
}

/**
 * This page's sort, among the ones its list offers. A page sorts its own rows by it and hands
 * TrackList the result, so a row, the page's Play button, and the queue all have one order
 * (`requirements/conventions.md` §4.1). Read in a `$derived`, it follows the URL.
 */
export function currentSort(fields: readonly SortField[]): TrackSort {
	return readSort(page.url.searchParams, fields);
}

/** `url` with `sort` in place of the sort it had, and everything else as it was. */
export function withSort(url: URL, sort: TrackSort): URL {
	const next = new URL(url);
	if (sort) {
		next.searchParams.set('sort', sort.field);
		next.searchParams.set('order', sort.order);
	} else {
		next.searchParams.delete('sort');
		next.searchParams.delete('order');
	}
	return next;
}

/**
 * Shows this page sorted by `sort`. Each click replaces the history entry rather than adding
 * one, so Back leaves the list instead of stepping through its sorts. A page whose load reads
 * the sort loads again; one that sorts what it already holds does not.
 */
export function showSorted(sort: TrackSort): Promise<void> {
	// eslint-disable-next-line svelte/no-navigation-without-resolve -- this page's own URL, already resolved
	return goto(withSort(page.url, sort), { replaceState: true, keepFocus: true, noScroll: true });
}
