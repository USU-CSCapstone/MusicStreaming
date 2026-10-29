import { describe, expect, it } from 'vitest';
import { tagTypeName, tagTypePlural } from './tags';

describe('a tag type', () => {
	const types = [
		{ key: 'genre', tagCount: 2 },
		{ key: 'acousticbrainz.energy', name: 'Energy', tagCount: 1 }
	];

	it('built in is named in our words', () => {
		expect([tagTypeName('genre', types), tagTypePlural('genre', types)]).toEqual([
			'Genre',
			'Genres'
		]);
		expect(tagTypePlural('mood')).toBe('Moods');
	});

	it('a plugin added is named as it named it, for one or more', () => {
		expect(tagTypeName('acousticbrainz.energy', types)).toBe('Energy');
		expect(tagTypePlural('acousticbrainz.energy', types)).toBe('Energy');
	});

	it('not listed is named by its key', () => {
		expect(tagTypeName('colour')).toBe('Colour');
		expect(tagTypePlural('colour')).toBe('Colour');
	});
});
