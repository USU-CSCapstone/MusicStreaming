import { describe, expect, it } from 'vitest';
import { ago, contextLabel, namedBy } from './kept';

describe('kept queues', () => {
	it('names a context, by its name when it has one', () => {
		expect(contextLabel({ type: 'album', id: '1' }, 'Signal')).toBe('Signal');
		expect(contextLabel({ type: 'album', id: '1' })).toBe('An album');
		expect(contextLabel({ type: 'search', query: 'x', section: 'tracks' })).toBe('Search “x”');
		expect(contextLabel({ type: 'library' })).toBe('Your library');
		expect(namedBy({ type: 'playlist', id: '4' })).toEqual({ kind: 'playlist', id: '4' });
		expect(namedBy({ type: 'library' })).toBeNull();
	});

	it('says how long ago a queue was last active', () => {
		const now = Date.parse('2026-11-02T12:00:00Z');
		expect(ago('2026-11-02T11:59:40Z', now)).toBe('just now');
		expect(ago('2026-11-02T11:15:00Z', now)).toBe('45 minutes ago');
		expect(ago('2026-10-30T12:00:00Z', now)).toBe('3 days ago');
	});
});
