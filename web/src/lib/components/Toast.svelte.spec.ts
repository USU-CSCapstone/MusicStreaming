import { page } from 'vitest/browser';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { toast, toaster } from '$lib/toast.svelte';
import Toast from './Toast.svelte';

afterEach(() => {
	toaster.dismiss();
});

describe('Toast.svelte', () => {
	it('announces the message, and takes its action', async () => {
		render(Toast);
		const run = vi.fn();
		toast('Removed 3 songs', { action: { label: 'Undo', run } });
		await expect.element(page.getByRole('status')).toHaveTextContent('Removed 3 songs');
		await page.getByRole('button', { name: 'Undo' }).click();
		expect(run).toHaveBeenCalledOnce();
		await expect.element(page.getByText('Removed 3 songs')).not.toBeInTheDocument();
	});

	it('dismisses without the action', async () => {
		render(Toast);
		const run = vi.fn();
		const onclose = vi.fn();
		toast('Deleted “Road Trip”', { action: { label: 'Undo', run }, onclose });
		await page.getByRole('button', { name: 'Dismiss' }).click();
		expect(onclose).toHaveBeenCalledOnce();
		expect(run).not.toHaveBeenCalled();
		await expect.element(page.getByText('Deleted “Road Trip”')).not.toBeInTheDocument();
	});

	it('shows no action button without an action', async () => {
		render(Toast);
		toast('The playlist changed on another device.');
		await expect.element(page.getByRole('status')).toHaveTextContent('another device');
		expect(page.getByRole('button').elements()).toHaveLength(1);
	});

	it('holds open while focus is on it', async () => {
		render(Toast);
		toast('Removed', { duration: 300 });
		const dismiss = page.getByRole('button', { name: 'Dismiss' });
		await expect.element(dismiss).toBeInTheDocument();
		(dismiss.element() as HTMLElement).focus();
		await new Promise((r) => setTimeout(r, 600));
		await expect.element(page.getByText('Removed')).toBeInTheDocument();
		(document.activeElement as HTMLElement).blur();
		await expect.element(page.getByText('Removed')).not.toBeInTheDocument();
	});
});
