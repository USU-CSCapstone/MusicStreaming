import { describe as group, expect, it } from 'vitest';
import { describe } from './device';

group('describe', () => {
	it.each([
		[
			'Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0',
			false,
			'Firefox on Linux',
			'desktop'
		],
		[
			'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36 Edg/128.0.0.0',
			false,
			'Edge on Windows',
			'desktop'
		],
		[
			'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Mobile Safari/537.36',
			true,
			'Chrome on Android',
			'phone'
		],
		[
			'Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36',
			true,
			'Chrome on Android',
			'tablet'
		],
		[
			'Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1',
			true,
			'Safari on iOS',
			'phone'
		],
		// iPadOS asks for the desktop site, so only touch gives it away.
		[
			'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15',
			true,
			'Safari on iPadOS',
			'tablet'
		],
		[
			'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15',
			false,
			'Safari on macOS',
			'desktop'
		],
		['curl/8.9.1', false, 'Browser', 'desktop']
	])('%s', (ua, touch, name, type) => {
		expect(describe(ua, touch)).toEqual({ name, type, platform: 'web' });
	});
});
