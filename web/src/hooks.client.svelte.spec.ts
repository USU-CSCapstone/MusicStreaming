import { expect, it, vi } from 'vitest';
import type { HandleClientError } from '@sveltejs/kit';

vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: { signOut: () => {}, preconnect: () => {} }
}));
vi.mock('$lib/realtime.svelte', () => ({ realtime: { onSessionEnd: null } }));

const { handleError } = await import('./hooks.client');
const { ApiError } = await import('$lib/api/root');

const run = (error: unknown) =>
	(handleError as (input: Parameters<HandleClientError>[0]) => unknown)({
		error,
		event: {} as never,
		status: 500,
		message: 'Internal Error'
	});

it("passes a forbidden's code, message and detail on to the error page", () => {
	const error = new ApiError(
		403,
		'forbidden',
		'Only the owner can read the audit log.',
		undefined,
		'Only the owner can read the audit log.'
	);
	expect(run(error)).toEqual({
		message: 'Only the owner can read the audit log.',
		code: 'forbidden',
		detail: 'Only the owner can read the audit log.'
	});
});

it('passes a forbidden with no detail on without one', () => {
	expect(run(new ApiError(403, 'forbidden', 'Forbidden'))).toEqual({
		message: 'Forbidden',
		code: 'forbidden'
	});
});
