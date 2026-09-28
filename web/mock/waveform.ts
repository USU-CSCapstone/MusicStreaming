// The scanner's waveform blob, reshaped into the API's `Waveform`.

import type { Waveform } from '../src/lib/api/types.ts';

/**
 * `track_waveforms.data` is a version byte (1), a little-endian u16 bin count,
 * then that many peak bytes, then as many RMS bytes (`crates/scanner/src/types.rs`).
 * Returns null for a blob this version does not understand.
 */
export function decodeWaveformBlob(
	blob: Uint8Array
): { peaks: Uint8Array; rms: Uint8Array } | null {
	if (blob.length < 3 || blob[0] !== 1) return null;
	const n = blob[1] | (blob[2] << 8);
	if (blob.length < 3 + 2 * n) return null;
	return { peaks: blob.slice(3, 3 + n), rms: blob.slice(3 + n, 3 + 2 * n) };
}

/**
 * The API promises points already shaped for legibility (`requirements/playback.md` §3).
 * Scaling to the track's own loudest peak keeps quiet recordings from drawing as a flat
 * line without inventing structure: every point keeps its place and relative height.
 */
export function toApiWaveform(blob: Uint8Array): Waveform | null {
	const decoded = decodeWaveformBlob(blob);
	if (!decoded) return null;
	const { peaks } = decoded;
	const max = peaks.reduce((m, p) => Math.max(m, p), 0);
	const shaped = max === 0 ? peaks : peaks.map((p) => Math.round((p / max) * 255));
	return { pointCount: shaped.length, data: Buffer.from(shaped).toString('base64') };
}
