import { describe, expect, it, vi } from 'vitest';
import type { PlayerCommand } from './api/types';
import { carryOut, type Controls } from './remote';

/** A player with a track loaded, `playing` or not, whose `resume` does as `resumes`. */
function player(playing: boolean, resumes = Promise.resolve()) {
	return {
		loaded: () => true,
		playing: () => playing,
		resume: vi.fn(() => resumes),
		pause: vi.fn(),
		next: vi.fn(),
		previous: vi.fn(),
		seek: vi.fn(),
		setVolume: vi.fn()
	} satisfies Controls;
}

const id = '00000000-0000-4000-8000-000000000001';
const command = (c: Record<string, unknown>) => ({ commandId: id, ...c }) as PlayerCommand;

describe('carryOut', () => {
	it('carries out each command', async () => {
		const p = player(true);
		expect(await carryOut(p, command({ type: 'pause' }))).toBeNull();
		expect(await carryOut(p, command({ type: 'next' }))).toBeNull();
		expect(await carryOut(p, command({ type: 'previous' }))).toBeNull();
		expect(await carryOut(p, command({ type: 'seek', positionMs: 30_500 }))).toBeNull();
		const volume = command({ type: 'setVolume', deviceId: '1', volume: 0.25 });
		expect(await carryOut(p, volume)).toBeNull();
		expect(p.pause).toHaveBeenCalledOnce();
		expect(p.next).toHaveBeenCalledOnce();
		expect(p.previous).toHaveBeenCalledOnce();
		expect(p.seek).toHaveBeenCalledWith(30.5);
		expect(p.setVolume).toHaveBeenCalledWith(0.25);
	});

	it('plays only what is not already playing, and says when the browser will not', async () => {
		const playing = player(true);
		expect(await carryOut(playing, command({ type: 'play' }))).toBeNull();
		expect(playing.resume).not.toHaveBeenCalled();

		expect(await carryOut(player(false), command({ type: 'play' }))).toBeNull();
		const blocked = player(false, Promise.reject(new Error('NotAllowedError')));
		expect(await carryOut(blocked, command({ type: 'play' }))).toBe(
			'This device could not start playing.'
		);
		const abort = new DOMException('The play() request was interrupted', 'AbortError');
		const paused = player(false, Promise.reject(abort));
		expect(await carryOut(paused, command({ type: 'play' })), 'paused right after').toBeNull();
	});

	it('fails what there is nothing loaded for, or that it cannot do yet', async () => {
		const empty = { ...player(false), loaded: () => false };
		expect(await carryOut(empty, command({ type: 'pause' }))).toBe(
			'Nothing is loaded on this device.'
		);
		const volume = command({ type: 'setVolume', deviceId: '1', volume: 0.5 });
		expect(await carryOut(empty, volume), 'volume belongs to the device').toBeNull();
		const transfer = command({ type: 'transfer', deviceId: '2' });
		expect(await carryOut(player(true), transfer)).toBe('This device cannot do that yet.');
	});
});
