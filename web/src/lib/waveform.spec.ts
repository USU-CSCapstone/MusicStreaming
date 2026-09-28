import { describe, expect, it } from 'vitest';
import { bars, decodeWaveform } from './waveform';

describe('decodeWaveform', () => {
	it('decodes base64 points to bytes', () => {
		const data = btoa(String.fromCharCode(0, 128, 255));
		expect([...decodeWaveform({ pointCount: 3, data })]).toEqual([0, 128, 255]);
	});
});

describe('bars', () => {
	it('keeps each bar’s loudest point, scaled to 0–1', () => {
		expect(bars(new Uint8Array([0, 255, 51, 102]), 2)).toEqual([1, 0.4]);
	});

	it('stretches fewer points across more bars', () => {
		expect(bars(new Uint8Array([255, 0]), 4)).toEqual([1, 1, 0, 0]);
	});

	it('is empty without points', () => {
		expect(bars(new Uint8Array(), 10)).toEqual([]);
	});
});
