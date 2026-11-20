import { page, userEvent } from 'vitest/browser';
import { expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { render } from 'vitest-browser-svelte';
import ConfirmDialog from './ConfirmDialog.svelte';

const body = createRawSnippet(() => ({ render: () => '<p>This signs them out everywhere.</p>' }));

/** The dialog with an `action` that records what it was given, plus the cancel handler. */
function open(props: { password?: boolean; action?: (password: string) => Promise<void> } = {}) {
	const action = vi.fn(props.action ?? (async () => {}));
	const onclose = vi.fn();
	render(ConfirmDialog, {
		title: 'Suspend ada?',
		confirmLabel: 'Suspend',
		danger: true,
		password: props.password,
		action,
		onclose,
		children: body
	} as never);
	return { action, onclose };
}

it('shows its title and body, and moves focus into the dialog', async () => {
	open();
	await expect.element(page.getByRole('dialog', { name: 'Suspend ada?' })).toBeVisible();
	await expect.element(page.getByText('This signs them out everywhere.')).toBeVisible();
	expect(document.querySelector('dialog')?.contains(document.activeElement)).toBe(true);
});

it('runs the action on confirm and then closes', async () => {
	const { action, onclose } = open();
	await page.getByRole('button', { name: 'Suspend' }).click();
	await vi.waitFor(() => expect(onclose).toHaveBeenCalled());
	expect(action).toHaveBeenCalledTimes(1);
});

it('stays open and shows the error when the action fails', async () => {
	const { onclose } = open({
		action: async () => {
			throw new Error('Only the owner can do this to an admin’s account.');
		}
	});
	await page.getByRole('button', { name: 'Suspend' }).click();
	await expect.element(page.getByRole('alert')).toHaveTextContent(/Only the owner/);
	await expect.element(page.getByRole('dialog')).toBeVisible();
	expect(onclose).not.toHaveBeenCalled();
});

it('cancels with the button or Escape without acting', async () => {
	const first = open();
	await page.getByRole('button', { name: 'Cancel' }).click();
	expect(first.onclose).toHaveBeenCalledTimes(1);
	await userEvent.keyboard('{Escape}');
	expect(first.onclose).toHaveBeenCalledTimes(2);
	expect(first.action).not.toHaveBeenCalled();
});

it('asks for the password when told to, and hands it to the action', async () => {
	const { action } = open({ password: true });
	const field = page.getByLabelText('Your password');
	await expect.element(field).toHaveAttribute('autocomplete', 'current-password');
	await field.fill('hunter2');
	await page.getByRole('button', { name: 'Suspend' }).click();
	await vi.waitFor(() => expect(action).toHaveBeenCalledWith('hunter2'));
});

it('does not run without the password when one is asked for', async () => {
	const { action } = open({ password: true });
	await page.getByRole('button', { name: 'Suspend' }).click();
	expect(action).not.toHaveBeenCalled();
});
