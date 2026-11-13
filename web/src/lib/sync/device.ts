// Where the changes mutations make on the device are made: the device's catalog, in the catalog
// worker of whichever tab owns it (`catalog/client.svelte.ts`). The worker keeps each change's undo
// in the library's file, takes them all back before it applies the server's rows and makes those
// still in the log again after (`catalog/sync.ts`), so a change shows until it is answered.

import { catalog } from '$lib/catalog/client.svelte';
import type { Local } from './mutation';

/** What the sender asks of the device. */
export type Device = {
	/**
	 * Makes those of `local`'s writes the device has not made, for `user`; settles once done, or
	 * once the device found it cannot, as where the library is not on it. With `answered`, the
	 * server answered the mutation that made them, as it leaves the log: the device makes none, and
	 * keeps making those it holds again through every rebase until the account feed brings them, so
	 * nothing answered vanishes while no feed comes.
	 */
	apply(user: string, local: Local, answered?: boolean): Promise<void>;
	/**
	 * Takes back every change made to `library` for `user` and makes those still in the log again,
	 * as after a mutation left the log unsent.
	 */
	rebase(user: string, library: string): Promise<void>;
	/**
	 * Hears that the server answered a write to `playlistId` of `library` with it at `version`, once
	 * the write has left the log, so one the device made twice is made once (`Definition.confirmed`).
	 */
	confirmed(user: string, library: string, playlistId: string, version: number): Promise<void>;
};

/** The device's catalog. Where none answers, nothing is made: reads go to the server then. */
export const catalogDevice: Device = {
	apply: (user, local, answered) => catalog.mutate(user, local, answered),
	rebase: (user, library) => catalog.rebase(user, library),
	confirmed: (user, library, playlistId, version) =>
		catalog.confirmed(user, library, playlistId, version)
};
