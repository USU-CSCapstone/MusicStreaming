// The spike's test plugin in JavaScript: what a "fifty-line plugin" author pays.

import { tracks } from 'jewelcase:spike/library@0.1.0';
import { get, set } from 'jewelcase:spike/state@0.1.0';

let count = 0n;

export function noop() {}

export function echo(s) {
	return s;
}

export function scanTitles(needle, batch) {
	let found = 0;
	let offset = 0;
	for (;;) {
		const page = tracks(offset, batch);
		if (page.length === 0) return found;
		for (const t of page) if (t.title.includes(needle)) found++;
		offset += page.length;
	}
}

export function counter() {
	count += 1n;
	return count;
}

export function persistedCounter() {
	const bytes = get('counter');
	let n = bytes ? new DataView(bytes.buffer, bytes.byteOffset, 8).getBigUint64(0, true) : 0n;
	n += 1n;
	const out = new Uint8Array(8);
	new DataView(out.buffer).setBigUint64(0, n, true);
	set('counter', out);
	return n;
}

export function spin() {
	let x = 0;
	for (;;) x = (x + 1) | 0;
}

export function crash() {
	throw new Error('plugin crashed on purpose');
}

export function hog(mb) {
	const held = [];
	for (let i = 0; i < mb; i++) held.push(new Uint8Array(1 << 20).fill(1));
	return held.length;
}
