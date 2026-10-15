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
		hasSettings: false,
		libraries: [
			{
				libraryId: LIB.id,
				enabled: false,
				autoDisabled: false,
				disabledReason: null,
				granted: [],
				missingRequired: ['libraryRead', 'network'],
				lastRun: null
			}
		],
		hostLimits: [],
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

	it('shows every rate limit it declares, flagging one every plugin keeps to', async () => {
		const network = {
			permission: 'network' as const,
			required: true,
			reason: 'To look things up.',
			destinations: ['musicbrainz.org', 'lrclib.net'],
			rateLimits: { 'lrclib.net': '2/s', 'musicbrainz.org': { limit: '1/s', per: 'host' as const } }
		};
		const limit = { permission: 'hostRateLimits' as const, required: false, reason: 'One limit.' };
		setup(plugin({ permissions: [network, limit] }));
		await expect
			.element(page.getByText('lrclib.net: 2 per second, for this plugin alone'))
			.toBeVisible();
		await expect
			.element(page.getByText(/musicbrainz.org: 1 per second, for every plugin, so it can slow/))
			.toBeVisible();
		const checkbox = page.getByRole('checkbox', { name: /Limit a service for every plugin/ });
		await expect.element(checkbox).not.toBeChecked();
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
