import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TOAST_MS, toast, toaster } from './toast.svelte';

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	toaster.dismiss();
	vi.useRealTimers();
});

describe('toast', () => {
	it('shows a message until it times out', () => {
		const onclose = vi.fn();
		toast('Removed 3 songs', { onclose });
		expect(toaster.current?.message).toBe('Removed 3 songs');
		vi.advanceTimersByTime(TOAST_MS - 1);
		expect(toaster.current).not.toBeNull();
		vi.advanceTimersByTime(1);
		expect(toaster.current).toBeNull();
		expect(onclose).toHaveBeenCalledOnce();
	});

	it('takes its own duration', () => {
		toast('Saved', { duration: 1000 });
		vi.advanceTimersByTime(1000);
		expect(toaster.current).toBeNull();
	});

	it('runs its action instead of closing quietly', () => {
		const run = vi.fn();
		const onclose = vi.fn();
		toast('Removed', { action: { label: 'Undo', run }, onclose });
		toaster.act();
		expect(run).toHaveBeenCalledOnce();
		expect(onclose).not.toHaveBeenCalled();
		expect(toaster.current).toBeNull();
		vi.advanceTimersByTime(TOAST_MS);
		expect(onclose).not.toHaveBeenCalled();
	});

	it('closes quietly when dismissed', () => {
		const run = vi.fn();
		const onclose = vi.fn();
		toast('Removed', { action: { label: 'Undo', run }, onclose });
		toaster.dismiss();
		expect(run).not.toHaveBeenCalled();
		expect(onclose).toHaveBeenCalledOnce();
	});

	it('replaces the one showing, which closes', () => {
		const first = vi.fn();
		toast('First', { onclose: first });
		toast('Second');
		expect(first).toHaveBeenCalledOnce();
		expect(toaster.current?.message).toBe('Second');
		// The replaced toast's timer is gone with it.
		vi.advanceTimersByTime(TOAST_MS - 1);
		expect(toaster.current?.message).toBe('Second');
	});

	it('dismisses only its own toast', () => {
		const dismissFirst = toast('First');
		toast('Second');
		dismissFirst();
		expect(toaster.current?.message).toBe('Second');
	});
});

describe('holding', () => {
	it('stops the clock while held, and resumes it with the time left', () => {
		toast('Removed');
		vi.advanceTimersByTime(4000);
		toaster.hold();
		vi.advanceTimersByTime(60_000);
		expect(toaster.current).not.toBeNull();
		toaster.release();
		vi.advanceTimersByTime(TOAST_MS - 4000 - 1);
		expect(toaster.current).not.toBeNull();
		vi.advanceTimersByTime(1);
		expect(toaster.current).toBeNull();
	});

	it('stays held until pointer and focus have both left', () => {
		toast('Removed');
		toaster.hold();
		toaster.hold();
		toaster.release();
		vi.advanceTimersByTime(60_000);
		expect(toaster.current).not.toBeNull();
		toaster.release();
		vi.advanceTimersByTime(TOAST_MS);
		expect(toaster.current).toBeNull();
	});

	it('ignores a release with nothing held', () => {
		toast('Removed');
		toaster.release();
		toaster.release();
		vi.advanceTimersByTime(TOAST_MS);
		expect(toaster.current).toBeNull();
	});

	it('starts each toast unheld', () => {
		toast('First');
		toaster.hold();
		toast('Second');
		vi.advanceTimersByTime(TOAST_MS);
		expect(toaster.current).toBeNull();
	});
});
