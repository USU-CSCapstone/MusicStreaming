import { describe, expect, it } from 'vitest';
import { SearchSettler, STAND_MS, type Selected } from './recent';

function settler() {
	let now = 0;
	const recorded: [string, string, Selected | undefined, boolean][] = [];
	const s = new SearchSettler(
		(libraryId, query, selected, keepalive) =>
			recorded.push([libraryId, query, selected, keepalive]),
		() => now
	);
	return { s, recorded, wait: (ms: number) => (now += ms) };
}

const album: Selected = { type: 'album', id: '101' };

describe('SearchSettler', () => {
	it('records nothing while the user types', () => {
		const { s, recorded, wait } = settler();
		for (const q of ['b', 'bo', 'bon']) {
			s.shown('1', q);
			wait(100);
		}
		expect(recorded).toEqual([]);
	});

	it('records a search left after it stood, even one that found nothing', () => {
		const { s, recorded, wait } = settler();
		s.shown('1', 'bon');
		wait(STAND_MS);
		s.leave();
		expect(recorded).toEqual([['1', 'bon', undefined, false]]);
	});

	it('does not record a search left before it stood', () => {
		const { s, recorded, wait } = settler();
		s.shown('1', 'bon');
		wait(STAND_MS - 1);
		s.leave();
		expect(recorded).toEqual([]);
	});

	it('records a submitted search once, and again only to add the result acted on', () => {
		const { s, recorded } = settler();
		s.shown('1', ' bon iver ');
		s.settle();
		s.settle();
		s.settle(album);
		s.leave();
		expect(recorded).toEqual([
			['1', 'bon iver', undefined, false],
			['1', 'bon iver', album, false]
		]);
	});

	it('records with keepalive when the page is closing', () => {
		const { s, recorded, wait } = settler();
		s.shown('1', 'bon');
		wait(STAND_MS);
		s.leave(true);
		expect(recorded).toEqual([['1', 'bon', undefined, true]]);
	});

	it('treats the same words searched again after leaving as a new search', () => {
		const { s, recorded } = settler();
		s.shown('1', 'bon');
		s.settle();
		s.leave();
		s.shown('1', 'bon');
		s.settle();
		expect(recorded).toHaveLength(2);
	});

	it('records nothing for an empty box', () => {
		const { s, recorded, wait } = settler();
		s.shown('1', '   ');
		wait(STAND_MS);
		s.settle();
		s.leave();
		expect(recorded).toEqual([]);
	});
});
