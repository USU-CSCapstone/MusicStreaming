import type { Waveform } from './api/types';

/** The API's base64 points as bytes, 0–255, already shaped for legibility by the server. */
export function decodeWaveform(w: Waveform): Uint8Array {
	const bin = atob(w.data);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}

/**
 * `points` resampled to `bars` values in 0–1, taking each bar's loudest point so
 * short transients stay visible at any width.
 */
export function bars(points: Uint8Array, bars: number): number[] {
	if (points.length === 0 || bars <= 0) return [];
	const out: number[] = [];
	for (let b = 0; b < bars; b++) {
		const start = Math.floor((b * points.length) / bars);
		const end = Math.max(start + 1, Math.floor(((b + 1) * points.length) / bars));
		let peak = 0;
		for (let i = start; i < end && i < points.length; i++) peak = Math.max(peak, points[i]);
		out.push(peak / 255);
	}
	return out;
}
