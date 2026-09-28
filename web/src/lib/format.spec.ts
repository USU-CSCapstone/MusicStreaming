import { describe, expect, it } from 'vitest';
import { albumTitle, artistName, formatDuration, formatLength, releaseYear } from './format';

describe('formatDuration', () => {
	it('formats minutes and seconds from microseconds', () => {
		expect(formatDuration(0)).toBe('0:00');
		expect(formatDuration(5_000_000)).toBe('0:05');
		expect(formatDuration(3 * 60_000_000 + 7_900_000)).toBe('3:07');
	});

	it('adds hours from an hour up', () => {
		expect(formatDuration(3600_000_000 + 65_000_000)).toBe('1:01:05');
	});

	it('never goes negative', () => {
		expect(formatDuration(-1)).toBe('0:00');
	});
});

describe('formatLength', () => {
	it('rounds to minutes, with hours when long', () => {
		expect(formatLength(125_000_000)).toBe('2 min');
		expect(formatLength(60 * 60_000_000)).toBe('1 hr');
		expect(formatLength(65 * 60_000_000)).toBe('1 hr 5 min');
	});
});

describe('unknown values', () => {
	it('names untagged albums and artists', () => {
		expect(albumTitle(null)).toBe('Unknown Album');
		expect(artistName(null)).toBe('Unknown Artist');
		expect(albumTitle('Low Country')).toBe('Low Country');
	});

	it('takes the year from a partial date', () => {
		expect(releaseYear('2021-03-12')).toBe('2021');
		expect(releaseYear('2019')).toBe('2019');
		expect(releaseYear(null)).toBeNull();
	});
});
