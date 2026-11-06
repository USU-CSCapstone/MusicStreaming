import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { marks } from './marks.svelte';
import { toaster } from './toast.svelte';

const track = { id: '5', title: 'Signal', liked: false };

let requests: string[];
let status: number;
/** Answers each request with `status`, once it settles, which it does at once by default. */
let release: Promise<void>;
const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
	const path = new URL(String(input), 'http://test').pathname.replace(/^.*\/api\/v1/, '');
	requests.push(`${init.method} ${path}`);
	const answer = status;
	await release;
	return answer === 204
		? new Response(null, { status: answer })
		: new Response(JSON.stringify({ status: answer, code: 'internal', detail: 'Try later.' }), {
				status: answer
			});
}) as typeof globalThis.fetch;

/** Holds every answer until the returned function lets them go. */
function hold(): () => void {
	let open = () => {};
	release = new Promise((resolve) => (open = resolve));
	return open;
}

/** Whether track 5 of library 1 shows as liked, and as hidden. */
const shown = (read = track) => [marks.liked('1', read), marks.hidden('1', '5')];

beforeEach(() => {
	marks.clear();
	requests = [];
	status = 204;
	release = Promise.resolve();
});

afterEach(() => toaster.dismiss());

describe('marks', () => {
	it('tells who listens of each hide and unhide shown, and of one undone', async () => {
		const heard: string[] = [];
		const stop = marks.onHidden((library, id, hidden) => heard.push(`${library}/${id} ${hidden}`));
		await marks.hide(fetch, '1', track, true);
		await marks.like(fetch, '1', track, true);
		marks.receive({ type: 'trackHidden', libraryId: '1', trackId: '6', hidden: true });
		// Already showing as not hidden, so nothing changed.
		await marks.like(fetch, '1', track, true);
		status = 500;
		await marks.hide(fetch, '1', track, true);
		stop();
		marks.receive({ type: 'trackHidden', libraryId: '1', trackId: '7', hidden: true });
		expect(heard).toEqual(['1/5 true', '1/5 false', '1/6 true', '1/5 true', '1/5 false']);
	});

	it('shows a track as it was read until a change says otherwise', async () => {
		expect(marks.liked('1', track)).toBe(false);
		expect(marks.liked('1', { ...track, liked: true })).toBe(true);
		const liking = marks.like(fetch, '1', track, true);
		expect(marks.liked('1', track)).toBe(true);
		await liking;
		expect(requests).toEqual(['PUT /libraries/1/tracks/5/like']);
		expect(marks.liked('2', track)).toBe(false);

		await marks.like(fetch, '1', { ...track, liked: true }, false);
		expect(marks.liked('1', { ...track, liked: true })).toBe(false);
		expect(requests.at(-1)).toBe('DELETE /libraries/1/tracks/5/like');
	});

	it('leaves out of a list what was hidden since it was read', async () => {
		const read = [track, { ...track, id: '6' }];
		expect(marks.unhidden('1', read)).toEqual(read);
		await marks.hide(fetch, '1', track, true);
		expect(marks.unhidden('1', read).map((t) => t.id)).toEqual(['6']);
		expect(marks.hidden('1', '6', true)).toBe(true);
	});

	it('hides and unhides', async () => {
		await marks.hide(fetch, '1', track, true);
		expect(marks.hidden('1', '5')).toBe(true);
		expect(requests).toEqual(['PUT /libraries/1/tracks/5/hidden']);
		await marks.hide(fetch, '1', track, false);
		expect(marks.hidden('1', '5')).toBe(false);
		expect(requests.at(-1)).toBe('DELETE /libraries/1/tracks/5/hidden');
	});

	it('undoes a change the server refuses, and says why', async () => {
		status = 500;
		await marks.like(fetch, '1', track, true);
		expect(marks.liked('1', track)).toBe(false);
		expect(toaster.current?.message).toBe('Couldn’t like “Signal”. Try later.');
		await marks.hide(fetch, '1', track, true);
		expect(marks.hidden('1', '5')).toBe(false);
	});

	it('likes unhide and hides unlike, here and as the server tells of them', async () => {
		await marks.hide(fetch, '1', track, true);
		await marks.like(fetch, '1', track, true);
		expect(shown()).toEqual([true, false]);

		marks.receive({ type: 'trackHidden', libraryId: '1', trackId: '5', hidden: true });
		expect(shown()).toEqual([false, true]);
		marks.receive({ type: 'trackLiked', libraryId: '1', trackId: '5', liked: true });
		expect(shown()).toEqual([true, false]);
		marks.receive({ type: 'trackLiked', libraryId: '1', trackId: '5', liked: false });
		expect(shown({ ...track, liked: true })).toEqual([false, false]);
	});

	it('takes no echo of an older change while a newer one is under way', async () => {
		const open = hold();
		const liking = marks.like(fetch, '1', track, true);
		const unliking = marks.like(fetch, '1', track, false);
		// The first like's echo, arriving as the unlike is sent.
		marks.receive({ type: 'trackLiked', libraryId: '1', trackId: '5', liked: true });
		expect(marks.liked('1', track)).toBe(false);
		open();
		await Promise.all([liking, unliking]);
		expect(marks.liked('1', track)).toBe(false);
	});

	it('goes back to what the server confirmed, never to a change it refused', async () => {
		const open = hold();
		status = 500;
		const liking = marks.like(fetch, '1', track, true);
		const hiding = marks.hide(fetch, '1', track, true);
		open();
		await Promise.all([liking, hiding]);
		expect(shown()).toEqual([false, false]);
	});

	it('leaves a later change in place when only an earlier one is refused', async () => {
		const open = hold();
		status = 500;
		const liking = marks.like(fetch, '1', track, true);
		status = 204;
		const hiding = marks.hide(fetch, '1', track, true);
		open();
		await Promise.all([liking, hiding]);
		expect(shown()).toEqual([false, true]);
	});

	it('unlikes every track of a library whose Liked Songs was deleted', async () => {
		const read = { ...track, liked: true };
		await marks.like(fetch, '1', { ...track, id: '6' }, true);
		marks.unlikeAll('1');
		expect([marks.liked('1', read), marks.liked('1', { ...read, id: '6' })]).toEqual([
			false,
			false
		]);
		expect(marks.liked('2', read)).toBe(true);
		await marks.like(fetch, '1', track, true);
		expect(marks.liked('1', read)).toBe(true);
	});

	it('knows Liked Songs deleted elsewhere once it has seen which playlist it is', () => {
		const read = { ...track, liked: true };
		marks.receive({
			type: 'playlist',
			libraryId: '1',
			playlistId: 'p9',
			version: 2,
			playlist: null
		});
		expect(marks.liked('1', read)).toBe(true);
		marks.note([
			{ id: 'p9', libraryId: '1', kind: 'liked' },
			{ id: 'p8', libraryId: '1', kind: 'saved' }
		]);
		marks.receive({
			type: 'playlist',
			libraryId: '1',
			playlistId: 'p8',
			version: 2,
			playlist: null
		});
		expect(marks.liked('1', read)).toBe(true);
		marks.receive({
			type: 'playlist',
			libraryId: '1',
			playlistId: 'p9',
			version: 2,
			playlist: null
		});
		expect(marks.liked('1', read)).toBe(false);
	});
});
