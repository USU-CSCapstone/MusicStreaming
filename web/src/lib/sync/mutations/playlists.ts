// Playlist creates, deletes, edits and appends (`requirements/playlists.md` §4–§5): each playlist
// is a scope, so its changes are sent one at a time in the order they were made. An edit names positions, so it is
// sent against the version the edit before it produced, which a device's own edits never
// conflict with; if another device changed the playlist meanwhile, the edit still goes where the
// server's items are the ones it was made on, and is refused otherwise, with every later edit made
// on it. An append names no positions, so a conflict is met by reading the version again and
// sending once more. The device makes each change at once through the server's own code.

import {
	ApiError,
	createPlaylist as postPlaylist,
	deletePlaylist as removePlaylist,
	editPlaylist,
	getPlaylist,
	listPlaylistItems
} from '$lib/api/client';
import type { Playable, Playlist, PlaylistEdit, PlaylistItem } from '$lib/api/types';
import { fingerprint } from '../fingerprint';
import type { Definition, Fetch, Writes } from '../mutation';

/** What the user is told when another device's change to a playlist refused theirs. */
export const CONFLICT_MESSAGE =
	'This playlist changed on another device, so your change wasn’t applied.';

/** How many times an append is sent again on a newer version before it is given up. */
const REBASES = 3;

/** An edit without its base version, which is filled in when it is sent. */
export type PlaylistChange = Omit<PlaylistEdit, 'baseVersion'>;

/** A playlist and every item in it, at one version. */
export type LoadedPlaylist = { playlist: Playlist; items: PlaylistItem[] };

/** An edit of `playlistId`, made on the playlist at `baseVersion`. */
export type EditArgs = {
	libraryId: string;
	playlistId: string;
	baseVersion: number;
	change: PlaylistChange;
	/** The `fingerprint` of the items the edit was made on, which its positions name. */
	fingerprint?: string;
	/**
	 * The track IDs each add of `change` brings in, in order, for the device to apply it; none where
	 * an add names anything but tracks.
	 */
	tracks?: string[][];
	/** What finishes "Couldn’t …"; "save your change" by default. */
	what?: string;
};

/** A new playlist, which the client names with an ID of its own (`mintId`). */
export type CreateArgs = {
	libraryId: string;
	/** The playlist's ID, a positive 63-bit integer in decimal. */
	id: string;
	title: string;
	description?: string;
	/** What it starts with: songs, albums, artists or playlists, which the server expands. */
	items?: Playable[];
	/** The track IDs `items` expand to, for the device to apply it; none where not known. */
	tracks?: string[];
};

/** A playlist deleted. */
export type DeleteArgs = {
	libraryId: string;
	playlistId: string;
	title: string;
};

/** Songs, albums, artists or playlists added at the end of `playlistId`. */
export type AppendArgs = {
	libraryId: string;
	playlistId: string;
	items: Playable[];
	/** The track IDs `items` expand to, for the device to apply it; none where not known. */
	tracks?: string[];
	/** What finishes "Couldn’t …", such as `add to “Road Trip”`. */
	what: string;
};

/**
 * A playlist and every item, at one version, so positions and `baseVersion` agree. Tries again
 * when the playlist changes while it loads. With `fresh`, from the server alone, as the check of
 * an edit that conflicted needs: a device behind the server would give the version that conflicted.
 */
export async function loadPlaylist(
	fetch: Fetch,
	libraryId: string,
	playlistId: string,
	fresh = false
): Promise<LoadedPlaylist> {
	for (let attempt = 1; ; attempt++) {
		try {
			const items: PlaylistItem[] = [];
			let version: number | undefined;
			let cursor: string | undefined;
			do {
				const page = await listPlaylistItems(
					fetch,
					libraryId,
					playlistId,
					{ limit: 1000, cursor },
					fresh
				);
				version ??= page.version;
				items.push(...page.items);
				cursor = page.nextCursor ?? undefined;
			} while (cursor);
			const playlist = await getPlaylist(fetch, libraryId, playlistId, fresh);
			if (playlist.version === version) return { playlist, items };
		} catch (e) {
			// `cursor_expired`: it changed between pages.
			if (!(e instanceof ApiError && e.status === 410)) throw e;
		}
		if (attempt === 3) throw new Error('The playlist kept changing while it loads.');
	}
}

/** Whether `error` is the server saying the playlist is at another version. */
const conflict = (error: unknown) => error instanceof ApiError && error.code === 'version_conflict';

/** The scope a playlist's changes are sent in, one after another. */
export const playlistScope = (playlistId: string) => `playlist:${playlistId}`;

/** The version the answer to the mutation before carried, if it was a playlist. */
const carriedVersion = (carried: unknown) => (carried as Playlist | undefined)?.version;

/** The ops of `change` as the device takes them, with each add's tracks from `tracks`. */
function deviceOps({ ops }: PlaylistChange, tracks: string[][]) {
	let add = 0;
	return ops?.map((op) => (op.op === 'add' ? { ...op, items: tracks[add++] } : op));
}

/** An edit of a playlist's details or items. */
export const editPlaylistMutation: Definition<EditArgs> = {
	scope: ({ playlistId }) => playlistScope(playlistId),
	async send(args, { fetch, key, carried }) {
		const { libraryId, playlistId, change } = args;
		const base = carriedVersion(carried) ?? args.baseVersion;
		try {
			return await editPlaylist(
				fetch,
				libraryId,
				playlistId,
				{ ...change, baseVersion: base },
				key
			);
		} catch (error) {
			// Another device changed it: the edit still goes if the items are the ones it was made on.
			if (!conflict(error) || !args.fingerprint) throw error;
			const loaded = await loadPlaylist(fetch, libraryId, playlistId, true);
			if (fingerprint(loaded.items) !== args.fingerprint) throw error;
			const baseVersion = loaded.playlist.version;
			return await editPlaylist(fetch, libraryId, playlistId, { ...change, baseVersion }, key);
		}
	},
	apply({ libraryId, playlistId, baseVersion, change, tracks }): Writes | undefined {
		const adds = change.ops?.filter((op) => op.op === 'add').length ?? 0;
		if (adds > 0 && tracks?.length !== adds) return undefined;
		const { title, description, pinned } = change;
		const ops = deviceOps(change, tracks ?? []);
		return {
			library: libraryId,
			calls: [
				{ name: 'editPlaylist', args: { playlistId, baseVersion, title, description, pinned, ops } }
			]
		};
	},
	// Its positions were worked out on the playlist as the edits before it left it.
	depends: () => true,
	what: (args) => args.what ?? 'save your change',
	refused: (_, code) => (code === 'version_conflict' ? CONFLICT_MESSAGE : undefined)
};

/** Songs added at the end of a playlist, wherever it has got to. */
export const appendToPlaylist: Definition<AppendArgs> = {
	scope: ({ playlistId }) => playlistScope(playlistId),
	async send({ libraryId, playlistId, items }, { fetch, key, carried }) {
		let version =
			carriedVersion(carried) ?? (await getPlaylist(fetch, libraryId, playlistId, true)).version;
		for (let rebased = 0; ; rebased++) {
			try {
				const ops = [{ op: 'add' as const, items }];
				return await editPlaylist(fetch, libraryId, playlistId, { ops, baseVersion: version }, key);
			} catch (error) {
				// The server keeps an answer only for an edit that applied, so the key is still free.
				if (!conflict(error) || rebased === REBASES) throw error;
				version = (await getPlaylist(fetch, libraryId, playlistId, true)).version;
			}
		}
	},
	// Made on no version, so the device takes it again over the server's rows of it.
	confirmed: ({ libraryId, playlistId }, answer) => ({
		library: libraryId,
		playlistId,
		version: (answer as Playlist).version
	}),
	apply: ({ libraryId, playlistId, tracks }) =>
		tracks && {
			library: libraryId,
			calls: [{ name: 'appendToPlaylist', args: { playlistId, items: tracks } }]
		},
	what: (args) => args.what
};

/**
 * A new playlist, made on the device at once and sent under the ID the client chose. Its later
 * edits are of its scope, so they follow it, on the version its answer carries.
 */
export const createPlaylist: Definition<CreateArgs> = {
	scope: ({ id }) => playlistScope(id),
	send({ libraryId, id, title, description, items }, { fetch, key }) {
		return postPlaylist(
			fetch,
			libraryId,
			{ id, title, ...(description && { description }), ...(items?.length && { items }) },
			key
		);
	},
	apply: ({ libraryId, id, title, description, items, tracks }): Writes | undefined =>
		items?.length && !tracks
			? undefined
			: {
					library: libraryId,
					calls: [{ name: 'createPlaylist', args: { id, title, description, items: tracks ?? [] } }]
				},
	what: ({ title }) => `create “${title}”`
};

/**
 * A playlist deleted, on the device at once. Logged held back, for the Undo that follows it, and
 * sent after the edits made to the playlist before it. One the server no longer has is as
 * deleted as was wanted.
 */
export const deletePlaylist: Definition<DeleteArgs> = {
	scope: ({ playlistId }) => playlistScope(playlistId),
	async send({ libraryId, playlistId }, { fetch, key }) {
		try {
			await removePlaylist(fetch, libraryId, playlistId, key);
		} catch (error) {
			if (!(error instanceof ApiError && error.status === 404)) throw error;
		}
	},
	apply: ({ libraryId, playlistId }) => ({
		library: libraryId,
		calls: [{ name: 'deletePlaylist', args: { playlistId } }]
	}),
	what: ({ title }) => `delete “${title}”`
};
