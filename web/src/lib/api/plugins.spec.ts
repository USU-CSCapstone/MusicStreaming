import { describe, expect, it } from 'vitest';
import { describeLimit, every, forEveryPlugin } from './plugins';

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

describe('describeLimit', () => {
	it('says how often, and whose requests it counts', () => {
		expect([
			describeLimit('1/s'),
			describeLimit({ limit: '60/min', per: 'user' }),
			describeLimit({ limit: '1000/h', per: 'host' })
		]).toEqual([
			'1 per second, for this plugin alone',
			'60 per minute, for each connected user',
			'1000 per hour, for every plugin'
		]);
	});
});

describe('forEveryPlugin', () => {
	it('is true only for a limit per host', () => {
		expect([
			forEveryPlugin('1/s'),
			forEveryPlugin({ limit: '1/s', per: 'user' }),
			forEveryPlugin({ limit: '1/s', per: 'host' })
		]).toEqual([false, false, true]);
	});
});
