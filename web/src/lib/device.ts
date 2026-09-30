// How this browser describes itself when it logs in: every login registers a device, named so
// the user recognizes it in their device list and can rename it (`requirements/users.md` §4).

import type { DeviceRegistration } from './api/types';

const BROWSERS: [RegExp, string][] = [
	[/Edg\//, 'Edge'],
	[/Firefox\/|FxiOS\//, 'Firefox'],
	[/Chrome\/|CriOS\//, 'Chrome'],
	[/Safari\//, 'Safari']
];

// iPadOS reports itself as a Mac, so iPads are told apart by touch in `thisDevice`.
const SYSTEMS: [RegExp, string][] = [
	[/Android/, 'Android'],
	[/iPhone|iPad/, 'iOS'],
	[/Mac OS X/, 'macOS'],
	[/Windows/, 'Windows'],
	[/CrOS/, 'ChromeOS'],
	[/Linux/, 'Linux']
];

const find = (list: [RegExp, string][], ua: string) => list.find(([re]) => re.test(ua))?.[1];

/** A registration such as "Firefox on Linux", from the user agent. */
export function describe(ua: string, touch = false): DeviceRegistration {
	const browser = find(BROWSERS, ua) ?? 'Browser';
	let system = find(SYSTEMS, ua);
	const iPad = /iPad/.test(ua) || (system === 'macOS' && touch);
	if (iPad) system = 'iPadOS';
	const tablet = iPad || /Tablet/.test(ua) || (system === 'Android' && !/Mobile/.test(ua));
	const type = tablet ? 'tablet' : /Mobi|iPhone/.test(ua) ? 'phone' : 'desktop';
	return { name: system ? `${browser} on ${system}` : browser, type, platform: 'web' };
}

export const thisDevice = (): DeviceRegistration =>
	describe(navigator.userAgent, navigator.maxTouchPoints > 1);
