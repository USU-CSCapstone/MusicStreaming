// Aliases for the API schemas the client uses. `schema.d.ts` is generated from
// `api/openapi.yaml` by `pnpm gen:api`; never edit it by hand.

import type { components } from './schema';

type Schemas = components['schemas'];

export type Id = Schemas['Id'];
export type Library = Schemas['Library'];
export type ImageRef = Schemas['ImageRef'];
export type ArtistRef = Schemas['ArtistRef'];
export type AlbumRef = Schemas['AlbumRef'];
export type TrackSummary = Schemas['TrackSummary'];
export type Track = Schemas['Track'];
export type TrackPage = Schemas['TrackPage'];
export type AlbumSummary = Schemas['AlbumSummary'];
export type Album = Schemas['Album'];
export type AlbumPage = Schemas['AlbumPage'];
export type ArtistSummary = Schemas['ArtistSummary'];
export type Artist = Schemas['Artist'];
export type ArtistPage = Schemas['ArtistPage'];
export type TagRef = Schemas['TagRef'];
export type TagsByType = Schemas['TagsByType'];
export type TagType = Schemas['TagType'];
export type Tag = Schemas['Tag'];
export type TagPage = Schemas['TagPage'];
export type Playlist = Schemas['Playlist'];
export type PlaylistPage = Schemas['PlaylistPage'];
export type PlaylistItem = Schemas['PlaylistItem'];
export type PlaylistItemPage = Schemas['PlaylistItemPage'];
export type PlaylistCreate = Schemas['PlaylistCreate'];
export type PlaylistEdit = Schemas['PlaylistEdit'];
export type PlaylistOp = Schemas['PlaylistOp'];
export type Playable = Schemas['Playable'];
export type Waveform = Schemas['Waveform'];
export type Lyrics = Schemas['Lyrics'];
export type PlaybackInfo = Schemas['PlaybackInfo'];
export type SearchResult = Schemas['SearchResult'];
export type SearchResponse = Schemas['SearchResponse'];
export type RecentSearch = Schemas['RecentSearch'];
export type SearchSectionType = Schemas['SearchSectionType'];
export type Problem = Schemas['Problem'];
export type ServerInfo = Schemas['ServerInfo'];
export type DeviceRegistration = Schemas['DeviceRegistration'];
export type SetupRequest = Schemas['SetupRequest'];
export type Session = Schemas['Session'];
export type LoginRequest = Schemas['LoginRequest'];
export type User = Schemas['User'];
export type Device = Schemas['Device'];

/** A cursor-paged list, as every list endpoint returns it. */
export type Page<T> = { items: T[]; nextCursor: string | null; total: number };
export type PlayReport = Schemas['PlayReport'];
export type PlayContext = Schemas['Context'];
export type RealtimeClientMessage = Schemas['RealtimeClientMessage'];
export type RealtimeServerMessage = Schemas['RealtimeServerMessage'];
export type PlayerState = Schemas['PlayerState'];
export type ProgressReport = Schemas['ProgressReport'];
export type PlayerCommand = Schemas['PlayerCommand'];
export type QueueSnapshot = Schemas['QueueSnapshot'];
/** A kept queue, as the list of them shows it. */
export type KeptQueue = Schemas['KeptQueue'];
/** A kept queue made active again: the snapshot, and where playback was left. */
export type RestoredQueue = Schemas['RestoredQueue'];
/** A queue's state, as a snapshot carries it. */
export type QueueState = Schemas['QueueState'];
/** A start as `POST /me/queue` takes it. */
export type StartQueue = Schemas['StartQueue'];
/** A page of the queue's rows, with each row's track. */
export type QueueRows = Schemas['QueueRows'];
/** An edit as the server applies it and `queueEdit` carries it. */
export type QueueEdit = Schemas['QueueEdit'];
/** An edit as a device sends it: an `add`'s source names `tracks` or a `recipe`. */
export type SentEdit = Schemas['EditQueue']['edit'];
/** How the server makes a source's list, as a start or an add names it. */
export type QueueRecipe = NonNullable<Extract<SentEdit, { type: 'add' }>['source']['recipe']>;
/** Where an add or move lands. */
export type QueueAnchor = Schemas['QueueAnchor'];
