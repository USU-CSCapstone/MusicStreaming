import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { marks } from './marks.svelte';
import { toaster } from './toast.svelte';

const track = { id: '5', title: 'Signal', liked: false };

let requests: string[];
let status: number;
/** What the server holds: the tracks liked, and those hidden. */
let held: { liked: string[]; hidden: string[] };
/** Answers each request with `status`, once it settles, which it does at once by default. */
let release: Promise<void>;
const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
	const path = new URL(String(input), 'http://test').pathname.replace(/^.*\/api\/v1/, '');
	const method = init.method ?? 'GET';
	requests.push(`${method} ${path}`);
	const answer = status;
	await release;
	if (method === 'GET' && answer === 204) {
		const summary = (id: string) => ({ ...track, id, liked: held.liked.includes(id) });
		const ids = path.endsWith('/hidden-tracks')
			? held.hidden
			: (new URL(String(input), 'http://test').searchParams.get('ids')?.split(',') ?? []);
		const items = ids.map(summary);
		return Response.json({ items, nextCursor: null, total: items.length });
	}
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

/** What the server sends as the connection opens. */
const hello = { type: 'hello', deviceId: '1', serverTime: '2026-01-01T00:00:00.000Z' } as const;

/** Whether track 5 of library 1 shows as liked, and as hidden. */
const shown = (read = track) => [marks.liked('1', read), marks.hidden('1', '5')];

beforeEach(() => {
	marks.clear();
	requests = [];
	status = 204;
	held = { liked: [], hidden: [] };
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

	it('tells of a refused like or unhide as the track was read', async () => {
		const heard: string[] = [];
		const stop = marks.onHidden((library, id, hidden) => heard.push(`${library}/${id} ${hidden}`));
		status = 500;
		// A song from a list, which shows none hidden, liked while the server is out of reach.
		await marks.like(fetch, '1', track, true);
		expect(heard.at(-1)).toBe('1/5 false');
		expect(heard).not.toContain('1/5 true');
		// One from Hidden Songs, unhidden there.
		await marks.hide(fetch, '1', { ...track, id: '6', hidden: true }, false);
		expect(heard.slice(-2)).toEqual(['1/6 false', '1/6 true']);
		stop();
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

	it('leaves a list the same list until a song in it is hidden', async () => {
		const read = [track, { ...track, id: '6' }];
		const seen: (typeof read)[] = [];
		const stop = $effect.root(() => {
			const shown = $derived(marks.unhidden('1', read));
			$effect(() => void seen.push(shown));
		});
		flushSync();
		await marks.like(fetch, '1', track, true);
		await marks.hide(fetch, '1', { ...track, id: '7' }, true);
		await marks.hide(fetch, '2', track, true);
		flushSync();
		expect(seen).toEqual([read]);
		expect(seen[0]).toBe(read);

		await marks.hide(fetch, '1', track, true);
		flushSync();
		expect(seen.map((list) => list.map((t) => t.id))).toEqual([['5', '6'], ['6']]);
		stop();
	});

	it('takes the server’s word on each marked track once the connection is back', async () => {
		// Hidden here, then unhidden elsewhere while the connection was down; liked here, and
		// still; liked here, then hidden elsewhere.
		const other = { ...track, id: '6' };
		const third = { ...track, id: '7' };
		await marks.hide(fetch, '1', track, true);
		await marks.like(fetch, '1', other, true);
		await marks.like(fetch, '1', third, true);
		held = { liked: ['6'], hidden: ['7'] };
		requests = [];
		await marks.receive(hello, fetch);
		expect(requests.toSorted()).toEqual([
			'GET /libraries/1/hidden-tracks',
			'GET /libraries/1/tracks'
		]);
		expect(shown()).toEqual([false, false]);
		expect(marks.unhidden('1', [track, other, third]).map((t) => t.id)).toEqual(['5', '6']);
		expect([marks.liked('1', other), marks.liked('1', third)]).toEqual([true, false]);
	});

	it('keeps what changes while it checks, and what it could not check', async () => {
		await marks.hide(fetch, '1', track, true);
		const open = hold();
		const checking = marks.receive(hello, fetch);
		marks.receive({ type: 'trackLiked', libraryId: '1', trackId: '5', liked: true });
		open();
		await checking;
		expect(shown(), 'told of after the check began').toEqual([true, false]);

		await marks.hide(fetch, '1', track, true);
		status = 500;
		await marks.receive(hello, fetch);
		expect(shown(), 'the server could not say').toEqual([false, true]);
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

	it('shows a like the server took though Liked Songs was deleted while it was sent', async () => {
		const open = hold();
		const liking = marks.like(fetch, '1', track, true);
		// Deleted elsewhere just as this like makes it again.
		marks.unlikeAll('1');
		expect(marks.liked('1', track)).toBe(false);
		open();
		await liking;
		expect(marks.liked('1', track)).toBe(true);
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
