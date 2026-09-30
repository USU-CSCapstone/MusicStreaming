import { describe, expect, it } from 'vitest';
import { every } from './plugins';

describe('every', () => {
	it('names an interval in its largest whole unit', () => {
		expect([every(5), every(90), every(60), every(360), every(1440), every(2880)]).toEqual([
			'5 minutes',
			'90 minutes',
			'hour',
			'6 hours',
			'day',
			'2 days'
		]);
	});
});
