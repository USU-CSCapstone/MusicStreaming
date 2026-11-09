// What a library's scans say to an admin: what is running, what is next, and how the last one
// ended (`requirements/scanning.md` §5). Shared by the admin page and the mock.

import type { Scan } from './types';

/** A library's scans by where each stands. */
export type ScanSummary = {
	/** The scan in progress, if any. */
	running: Scan | null;
	/** The scan that starts next: one queued while another runs, or one about to start. */
	next: Scan | null;
	/** The last scan to finish, whichever way it ended. */
	last: Scan | null;
};

/**
 * `adminListScans`' items, newest first, by where each stands. The oldest queued scan is next,
 * as the scanner works its queue in order.
 */
export function summarize(scans: Scan[]): ScanSummary {
	const queued = scans.filter((s) => s.state === 'queued');
	return {
		running: scans.find((s) => s.state === 'running') ?? null,
		next: queued.at(-1) ?? null,
		last: scans.find((s) => s.finishedAt !== null) ?? null
	};
}

/** Whether a scan is queued or running, so its progress is worth asking for again. */
export function underWay(scans: Scan[]): boolean {
	return scans.some((s) => s.state === 'queued' || s.state === 'running');
}

/** Why a scan started, as an admin reads it: "Scheduled scan". */
export const TRIGGER_LABELS: Record<Scan['trigger'], string> = {
	initial: 'First scan',
	watch: 'Scan of changed files',
	scheduled: 'Scheduled scan',
	manual: 'Manual scan',
	reconfigure: 'Rescan for new tag types',
	restore: 'Scan after a restore'
};
