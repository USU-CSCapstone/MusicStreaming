// What version 2 of the database kept in its lanes, as mutations of the log (`store.ts` moves them
// in its upgrade), and what it set aside, as mutations set aside. Each play, each track's latest
// mark, and each edit of a playlist chain the server had not confirmed becomes one mutation, under
// its own idempotency key where it had one, so an edit that applied before the upgrade is not
// applied twice. What the device shows of them is worked out by the sender when it takes them
// (`local`). Version 2's queue records, a start and edits not yet sent by each tab, are not moved:
// they were made on a queue of another shape, which cannot be replayed safely (`design/web.md`).

import type { PlayReport, PlaylistItem } from '$lib/api/types';
import { hashText } from './fingerprint';
import type { Mutation } from './mutation';
import type { MarkArgs } from './mutations/marks';
import type { AppendArgs, EditArgs, PlaylistChange } from './mutations/playlists';
import type { Stored } from './store';

/** What version 2's lanes held, by lane. */
export type Lanes = {
	plays: (Stored & { report: PlayReport })[];
	marks: (Stored & Omit<MarkArgs, 'likedId'>)[];
	playlists: (Stored & {
		libraryId: string;
		playlistId: string;
		version: number;
		/** The items the chain's first edit was made on, as positions see them, unhashed. */
		fingerprint: string;
		edits: { key: string; change: PlaylistChange; added: PlaylistItem[][] }[];
		sent: number;
		append?: boolean;
		what?: string;
	})[];
};

/**
 * The mutations a record version 2 set aside in `dead` becomes, the record kept under its lane's
 * name and its own key (`plays/<key>`): what its lane's record becomes, so it can be retried or
 * discarded as any mutation set aside. None for a record that is a mutation already.
 */
export function setAsideOf(rec: Stored & { name?: string }): Mutation[] | undefined {
	if (rec.name !== undefined) return undefined;
	const slash = rec.key.indexOf('/');
	const lane = rec.key.slice(0, slash);
	if (lane !== 'plays' && lane !== 'marks' && lane !== 'playlists') return undefined;
	return migrate({ [lane]: [{ ...rec, key: rec.key.slice(slash + 1) }] });
}

/** The parts of a mutation every record of a lane gives it as it was. */
const kept = ({ rev, owner, createdAt, tries }: Stored) => ({ rev, owner, createdAt, tries });

/**
 * The mutations `lanes` become, in the order they were made: a chain's edits by their order in it.
 */
export function migrate(lanes: Partial<Lanes>): Mutation[] {
	const mutations: Mutation[] = [];
	for (const rec of lanes.plays ?? []) {
		mutations.push({
			...kept(rec),
			key: `plays/${rec.key}`,
			name: 'play',
			args: rec.report,
			scope: 'plays',
			seq: rec.createdAt * 1000,
			idem: rec.rev
		});
	}
	for (const rec of lanes.marks ?? []) {
		const { library, track, liked, hidden, what } = rec;
		const args: MarkArgs = { library, track, liked, hidden, what };
		mutations.push({
			...kept(rec),
			key: `marks/${rec.key}`,
			name: 'mark',
			args,
			scope: `mark:${library}/${track}`,
			seq: rec.createdAt * 1000,
			idem: rec.rev
		});
	}
	for (const rec of lanes.playlists ?? []) {
		// As `playlistScope` makes it: the workers load this module, so it imports none of the page's.
		const scope = `playlist:${rec.playlistId}`;
		rec.edits.slice(rec.sent).forEach(({ key, change, added }, i) => {
			const base = {
				...kept(rec),
				key: `playlists/${rec.key}/${rec.sent + i}`,
				scope,
				seq: rec.createdAt * 1000 + i,
				idem: key
			};
			const { libraryId, playlistId } = rec;
			if (rec.append) {
				const args: AppendArgs = {
					libraryId,
					playlistId,
					items: change.ops?.[0]?.op === 'add' ? change.ops[0].items : [],
					what: rec.what ?? 'save your change'
				};
				mutations.push({ ...base, name: 'appendToPlaylist', args });
				return;
			}
			const tracks = added.map((items) => items.map((item) => item.track?.id ?? ''));
			const args: EditArgs = {
				libraryId,
				playlistId,
				// The first is made on what the server confirmed; each after on the one before it.
				baseVersion: rec.version + i,
				change,
				fingerprint: rec.sent === 0 && i === 0 ? hashText(rec.fingerprint) : undefined,
				tracks: tracks.flat().includes('') ? undefined : tracks
			};
			mutations.push({ ...base, name: 'editPlaylist', args });
		});
	}
	return mutations;
}
