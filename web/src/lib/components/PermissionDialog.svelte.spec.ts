import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Plugin } from '$lib/api/plugins';
import PermissionDialog from './PermissionDialog.svelte';

const LIB = { id: '11', name: 'Music' };

function plugin(overrides: Partial<Plugin> = {}): Plugin {
	return {
		id: 'lrclib-lyrics',
		name: 'LRCLIB Lyrics',
		version: '0.1.0',
		apiVersion: '0.2',
		source: { kind: 'file' },
		installedAt: '2026-09-29T00:00:00Z',
		permissions: [
			{ permission: 'libraryRead', required: true, reason: 'To find tracks without lyrics.' },
			{
				permission: 'network',
				required: true,
				reason: 'To fetch lyrics.',
				destinations: ['lrclib.net']
			},
			{ permission: 'libraryAdd', required: false, reason: 'To save .lrc files.' }
		],
		granted: [],
		libraries: [
			{
				libraryId: LIB.id,
				enabled: false,
				autoDisabled: false,
				disabledReason: null,
				granted: [],
				missingRequired: ['libraryRead', 'network']
			}
		],
		...overrides
	};
}

function setup(p = plugin()) {
	const onapprove = vi.fn();
	const oncancel = vi.fn();
	render(PermissionDialog, { plugin: p, libraries: [LIB], mode: 'install', onapprove, oncancel });
	return { onapprove, oncancel };
}

describe('PermissionDialog.svelte', () => {
	it('shows each request with its reason, and pre-approves nothing', async () => {
		setup();
		await expect.element(page.getByRole('dialog', { name: /Allow LRCLIB Lyrics/ })).toBeVisible();
		await expect.element(page.getByText('“To fetch lyrics.”')).toBeVisible();
		await expect.element(page.getByText('Only lrclib.net')).toBeVisible();
		for (const name of [/Network access/, /Read the library/, /Add files to the library/]) {
			await expect.element(page.getByRole('checkbox', { name })).not.toBeChecked();
		}
	});

	it('cannot enable until every required permission is checked', async () => {
		const { onapprove } = setup();
		const enable = page.getByRole('checkbox', { name: 'Enable in Music' });
		await expect.element(enable).toBeDisabled();
		await expect.element(page.getByText(/Needs Read the library and Network access/)).toBeVisible();

		await page.getByRole('checkbox', { name: /Read the library/ }).click();
		await page.getByRole('checkbox', { name: /Network access/ }).click();
		await expect.element(enable).toBeEnabled();
		await expect.element(enable).toBeChecked();

		await page.getByRole('button', { name: 'Approve selected' }).click();
		expect(onapprove).toHaveBeenCalledWith({
			grants: { granted: ['network'], libraries: [{ libraryId: '11', granted: ['libraryRead'] }] },
			enable: { '11': true }
		});
	});

	it('approves everything at once', async () => {
		const { onapprove } = setup();
		await page.getByRole('button', { name: 'Approve all' }).click();
		expect(onapprove).toHaveBeenCalledWith({
			grants: {
				granted: ['network'],
				libraries: [{ libraryId: '11', granted: ['libraryRead', 'libraryAdd'] }]
			},
			enable: { '11': true }
		});
	});

	it('does not enable when a required permission is left out', async () => {
		const { onapprove } = setup();
		await page.getByRole('checkbox', { name: /Read the library/ }).click();
		await page.getByRole('button', { name: 'Approve selected' }).click();
		expect(onapprove.mock.lastCall?.[0].enable).toEqual({ '11': false });
	});

	it('says when earlier grants are restored', async () => {
		setup(plugin({ granted: ['network'] }));
		await expect.element(page.getByText(/approved when it was last installed/)).toBeVisible();
		await expect.element(page.getByRole('checkbox', { name: /Network access/ })).toBeChecked();
	});

	it('cancels from the button', async () => {
		const { oncancel } = setup();
		await page.getByRole('button', { name: 'Cancel install' }).click();
		expect(oncancel).toHaveBeenCalled();
	});
});
