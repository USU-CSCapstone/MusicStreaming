// What a library's scans say to an admin: what is running, what is next, and how the last one
// ended (`requirements/scanning.md` §5), in the admin's words.

import { plural } from '../format';
import type { Scan } from './types';

/** A library's scans by where each stands. */
export type ScanSummary = {
	/** The scan in progress, if any. */
	running: Scan | null;
	/** The scans waiting to start, the next first: the scanner works its queue in order. */
	queued: Scan[];
	/** The last scan to finish, whichever way it ended. */
	last: Scan | null;
};

/** `adminListScans`' items, newest first, by where each stands. */
export function summarize(scans: Scan[]): ScanSummary {
	return {
		running: scans.find((s) => s.state === 'running') ?? null,
		queued: scans.filter((s) => s.state === 'queued').reverse(),
		last: scans.find((s) => s.finishedAt !== null) ?? null
	};
}

/** Whether a scan is queued or running, so its progress is worth asking for again. */
export function underWay(scans: Scan[]): boolean {
	return scans.some((s) => s.state === 'queued' || s.state === 'running');
}

/**
 * The queued scan of the whole library, if there is one: what a press of Scan now answers with,
 * queuing nothing more. A scan of changed files covers only their folders, so it is not one.
 */
export function waiting(scans: Scan[]): Scan | null {
	return scans.find((s) => s.state === 'queued' && s.trigger !== 'watch') ?? null;
}

/**
 * The scans to show while a press of Scan now is on its way: a manual scan queued, at once
 * (`requirements/general.md` §3.4), unless the press will answer with one already waiting.
 * The server's answer replaces it.
 */
export function withPressed(scans: Scan[]): Scan[] {
	if (waiting(scans)) return scans;
	const pressed: Scan = {
		id: 'pressed',
		trigger: 'manual',
		state: 'queued',
		startedAt: null,
		finishedAt: null,
		filesSeen: 0,
		filesProcessed: 0,
		added: 0,
		updated: 0,
		missing: 0,
		problems: 0,
		currentFolder: null
	};
	return [pressed, ...scans];
}

/** How far a running scan has got: "Read 1,200 of 1,234 files found so far". */
export function progress(scan: Scan): string {
	if (scan.filesSeen === 0) return 'Looking for files';
	return `Read ${scan.filesProcessed.toLocaleString()} of ${plural(scan.filesSeen, 'file')} found so far`;
}

/** What a scan found, by what changed: "3 added · 2 missing · 1 problem". Empty when nothing did. */
export function findings(scan: Scan): string {
	const parts: [number, string][] = [
		[scan.added, 'added'],
		[scan.updated, 'updated'],
		[scan.missing, 'missing']
	];
	const changed = parts.filter(([n]) => n > 0).map(([n, what]) => `${n.toLocaleString()} ${what}`);
	if (scan.problems > 0) changed.push(plural(scan.problems, 'problem'));
	return changed.join(' · ');
}

/** The folder a running scan is in, as an admin reads it: the root itself has no name of its own. */
export function folder(scan: Scan): string | null {
	if (scan.currentFolder === null) return null;
	return scan.currentFolder === '' ? 'the top folder' : scan.currentFolder;
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
