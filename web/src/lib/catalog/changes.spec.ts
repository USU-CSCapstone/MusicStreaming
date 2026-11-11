import { describe, expect, it } from 'vitest';
import { Changes } from './changes';

describe('changes', () => {
	it('tells a change to a library and kind after a count, and nothing else', () => {
		const changes = new Changes();
		changes.record('1', ['tracks']);
		const count = changes.count;
		expect(changes.since(count, '1', ['tracks'])).toBe(false);

		changes.record('1', ['albums', 'tags']);
		changes.record('2', ['tracks']);
		expect(changes.since(count, '1', ['tracks', 'playlists'])).toBe(false);
		expect(changes.since(count, '1', ['tags'])).toBe(true);
		expect(changes.since(count, '2', ['playlists'])).toBe(false);
		expect(changes.since(count, '2', ['tracks'])).toBe(true);
	});
});
