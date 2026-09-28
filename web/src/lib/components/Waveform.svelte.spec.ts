import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import Waveform from './Waveform.svelte';

describe('Waveform.svelte', () => {
	it('seeks from the keyboard without depending on the shape', async () => {
		const onseek = vi.fn();
		render(Waveform, {
			points: new Uint8Array([10, 200, 90]),
			position: 60,
			duration: 180,
			onseek
		});

		const slider = page.getByRole('slider', { name: 'Seek' });
		await expect.element(slider).toHaveAttribute('aria-valuetext', '1:00 of 3:00');
		await slider.click();
		onseek.mockClear();

		await userEvent.keyboard('{ArrowRight}');
		expect(onseek).toHaveBeenLastCalledWith(65);
		await userEvent.keyboard('{PageDown}');
		expect(onseek).toHaveBeenLastCalledWith(30);
		await userEvent.keyboard('{Home}');
		expect(onseek).toHaveBeenLastCalledWith(0);
	});

	it('draws a plain bar before analysis', async () => {
		const { container } = render(Waveform, {
			points: null,
			position: 0,
			duration: 10,
			onseek: () => {}
		});
		await expect.element(page.getByRole('slider')).toBeInTheDocument();
		expect(container.querySelector('svg')).toBeNull();
		expect(container.querySelector('.plain')).not.toBeNull();
	});

	it('cannot be seeked with nothing playing', async () => {
		const onseek = vi.fn();
		render(Waveform, { points: null, position: 0, duration: 0, onseek });
		await expect.element(page.getByRole('slider')).toHaveAttribute('aria-disabled', 'true');
		expect(onseek).not.toHaveBeenCalled();
	});
});
