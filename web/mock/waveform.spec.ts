import { describe, expect, it } from 'vitest';
import { decodeWaveformBlob, toApiWaveform } from './waveform';

/** A blob as `crates/scanner/src/types.rs` writes it. */
function blob(peaks: number[], rms: number[]): Uint8Array {
	return new Uint8Array([1, peaks.length & 0xff, peaks.length >> 8, ...peaks, ...rms]);
}

describe('decodeWaveformBlob', () => {
	it('splits peaks from RMS', () => {
		const d = decodeWaveformBlob(blob([10, 20, 30], [1, 2, 3]));
		expect([...d!.peaks]).toEqual([10, 20, 30]);
		expect([...d!.rms]).toEqual([1, 2, 3]);
	});

	it('reads a bin count past one byte', () => {
		const n = 300;
		const d = decodeWaveformBlob(blob(Array(n).fill(9), Array(n).fill(1)));
		expect(d!.peaks.length).toBe(n);
	});

	it('rejects unknown versions and short blobs', () => {
		expect(decodeWaveformBlob(new Uint8Array([2, 1, 0, 5, 5]))).toBeNull();
		expect(decodeWaveformBlob(new Uint8Array([1, 4, 0, 5]))).toBeNull();
	});
});

describe('toApiWaveform', () => {
	it('scales to the track’s own loudest peak, keeping relative heights', () => {
		const w = toApiWaveform(blob([0, 50, 100], [0, 0, 0]))!;
		expect(w.pointCount).toBe(3);
		expect([...Buffer.from(w.data, 'base64')]).toEqual([0, 128, 255]);
	});

	it('leaves silence silent', () => {
		const w = toApiWaveform(blob([0, 0], [0, 0]))!;
		expect([...Buffer.from(w.data, 'base64')]).toEqual([0, 0]);
	});
});
