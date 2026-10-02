// Aliases for the API schemas the client uses. `schema.d.ts` is generated from
// `api/openapi.yaml` by `pnpm gen:api`; never edit it by hand.

import type { components } from './schema';

type Schemas = components['schemas'];

export type Id = Schemas['Id'];
export type Library = Schemas['Library'];
export type ImageRef = Schemas['ImageRef'];
export type ArtistCredit = Schemas['ArtistCredit'];
export type AlbumRef = Schemas['AlbumRef'];
export type TrackSummary = Schemas['TrackSummary'];
export type TrackPage = Schemas['TrackPage'];
export type AlbumSummary = Schemas['AlbumSummary'];
export type Album = Schemas['Album'];
export type AlbumPage = Schemas['AlbumPage'];
export type ArtistSummary = Schemas['ArtistSummary'];
export type Artist = Schemas['Artist'];
export type ArtistPage = Schemas['ArtistPage'];
export type Playlist = Schemas['Playlist'];
export type PlaylistPage = Schemas['PlaylistPage'];
export type PlaylistItem = Schemas['PlaylistItem'];
export type PlaylistItemPage = Schemas['PlaylistItemPage'];
export type Waveform = Schemas['Waveform'];
export type Lyrics = Schemas['Lyrics'];
export type PlaybackInfo = Schemas['PlaybackInfo'];
export type SearchResult = Schemas['SearchResult'];
export type SearchResponse = Schemas['SearchResponse'];
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
