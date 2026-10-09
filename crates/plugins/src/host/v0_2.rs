//! Plugins built against the 0.2 contract (`wit/0.2/plugin.wit`), which keep loading
//! (`requirements/plugins.md` §1). Each 0.2 import passes the call on to the current one, and
//! each event goes to the plugin as 0.2 describes it.
//!
//! 0.2 differs only in lacking what 0.3 added: the `playing`, `searched`, `albums-changed`, and
//! `artists-changed` hooks and search activity, which a 0.2 manifest cannot ask for. Its records are the current ones under other names, copied field by field.

use super::bindings::jewelcase::plugin as now;
use super::bindings::v0_2::jewelcase::plugin as old;
use super::{Library, Run};

/// `From` one contract's record to the other's twin, field by field.
macro_rules! twin {
    ($from:ty => $to:ty { $($field:ident),* $(,)? }) => {
        impl From<$from> for $to {
            fn from(v: $from) -> Self {
                Self { $($field: v.$field.into()),* }
            }
        }
    };
}

twin!(now::library::Track => old::library::Track {
    id, title, artists, album_id, album, disc_number, track_number, release_date, isrc,
    duration_ms, has_lyrics, root, path,
});
twin!(now::library::Album => old::library::Album { id, title, artists, release_date, track_count });
twin!(now::library::Artist => old::library::Artist { id, name, album_count, track_count });
twin!(now::files::Entry => old::files::Entry { name, directory, size, modified_ms });
twin!(old::http::Request => now::http::Request { method, url, headers, body });
twin!(now::http::Response => old::http::Response { status, headers, body });
twin!(now::events::TracksChanged => old::events::TracksChanged { changed, removed });
twin!(now::events::ScanFinished => old::events::ScanFinished {
    files_seen, added, updated, moved, missing, problems,
});
twin!(now::events::Play => old::events::Play { track, started_at, listen_time_ms, end });

impl From<old::files::WriteMode> for now::files::WriteMode {
    fn from(mode: old::files::WriteMode) -> Self {
        match mode {
            old::files::WriteMode::Create => Self::Create,
            old::files::WriteMode::Replace => Self::Replace,
        }
    }
}

impl From<old::http::Method> for now::http::Method {
    fn from(method: old::http::Method) -> Self {
        use old::http::Method as M;
        match method {
            M::Get => Self::Get,
            M::Head => Self::Head,
            M::Post => Self::Post,
            M::Put => Self::Put,
            M::Patch => Self::Patch,
            M::Delete => Self::Delete,
        }
    }
}

impl From<now::events::PlayEnd> for old::events::PlayEnd {
    fn from(end: now::events::PlayEnd) -> Self {
        use now::events::PlayEnd as E;
        match end {
            E::Finished => Self::Finished,
            E::Skipped => Self::Skipped,
            E::Stopped => Self::Stopped,
        }
    }
}

impl From<now::settings::Value> for old::settings::Value {
    fn from(value: now::settings::Value) -> Self {
        use now::settings::Value as V;
        match value {
            V::Text(text) => Self::Text(text),
            V::Number(n) => Self::Number(n),
            V::Flag(flag) => Self::Flag(flag),
        }
    }
}

fn all<A, B: From<A>>(items: Vec<A>) -> Vec<B> {
    items.into_iter().map(B::from).collect()
}

/// A permission as 0.2 names it, or none for one added since.
fn permission(permission: now::host::Permission) -> Option<old::host::Permission> {
    use now::host::Permission as P;
    use old::host::Permission as O;
    Some(match permission {
        P::LibraryRead => O::LibraryRead,
        P::LibraryAdd => O::LibraryAdd,
        P::LibraryChange => O::LibraryChange,
        P::Network => O::Network,
        P::ListeningActivity => O::ListeningActivity,
        P::TracksChanged => O::TracksChanged,
        P::ScanFinished => O::ScanFinished,
        P::Schedule => O::Schedule,
        P::Played => O::Played,
        P::Playing | P::SearchActivity | P::Searched | P::AlbumsChanged | P::ArtistsChanged => {
            return None;
        }
    })
}

/// The event as 0.2 describes it, or none for one added since.
pub(super) fn event(event: &now::events::Event) -> Option<old::events::Event> {
    use now::events::Event as E;
    use old::events::Event as O;
    Some(match event.clone() {
        E::Run => O::Run,
        E::TracksChanged(changes) => O::TracksChanged(changes.into()),
        E::ScanFinished(totals) => O::ScanFinished(totals.into()),
        E::Scheduled => O::Scheduled,
        E::CheckSettings => O::CheckSettings,
        E::Played(plays) => O::Played(all(plays)),
        E::Playing(_)
        | E::Searched(_)
        | E::SearchesForgotten(_)
        | E::AlbumsChanged(_)
        | E::ArtistsChanged(_) => return None,
    })
}

impl<L: Library> old::host::Host for Run<L> {
    async fn granted(&mut self) -> Vec<old::host::Permission> {
        let granted = now::host::Host::granted(self).await;
        granted.into_iter().filter_map(permission).collect()
    }

    async fn log(&mut self, message: String) {
        now::host::Host::log(self, message).await
    }
}

impl<L: Library> old::library::Host for Run<L> {
    async fn tracks(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<old::library::Track>, String> {
        now::library::Host::tracks(self, after, limit).await.map(all)
    }

    async fn get_tracks(&mut self, ids: Vec<u64>) -> Result<Vec<old::library::Track>, String> {
        now::library::Host::get_tracks(self, ids).await.map(all)
    }

    async fn albums(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<old::library::Album>, String> {
        now::library::Host::albums(self, after, limit).await.map(all)
    }

    async fn artists(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<old::library::Artist>, String> {
        now::library::Host::artists(self, after, limit).await.map(all)
    }
}

impl<L: Library> old::files::Host for Run<L> {
    async fn roots(&mut self) -> Result<Vec<u64>, String> {
        now::files::Host::roots(self).await
    }

    async fn list(&mut self, root: u64, folder: String) -> Result<Vec<old::files::Entry>, String> {
        now::files::Host::list(self, root, folder).await.map(all)
    }

    async fn read(
        &mut self,
        root: u64,
        path: String,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        now::files::Host::read(self, root, path, offset, length).await
    }

    async fn write(
        &mut self,
        root: u64,
        path: String,
        contents: Vec<u8>,
        mode: old::files::WriteMode,
    ) -> Result<(), String> {
        now::files::Host::write(self, root, path, contents, mode.into()).await
    }

    async fn rename(&mut self, root: u64, from: String, to: String) -> Result<(), String> {
        now::files::Host::rename(self, root, from, to).await
    }

    async fn delete(&mut self, root: u64, path: String) -> Result<(), String> {
        now::files::Host::delete(self, root, path).await
    }
}

impl<L: Library> old::http::Host for Run<L> {
    async fn send(&mut self, request: old::http::Request) -> Result<old::http::Response, String> {
        now::http::Host::send(self, request.into()).await.map(Into::into)
    }
}

impl<L: Library> old::state::Host for Run<L> {
    async fn get(&mut self, key: String) -> Result<Option<Vec<u8>>, String> {
        now::state::Host::get(self, key).await
    }

    async fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        now::state::Host::set(self, key, value).await
    }

    async fn delete(&mut self, key: String) -> Result<(), String> {
        now::state::Host::delete(self, key).await
    }
}

impl<L: Library> old::settings::Host for Run<L> {
    async fn get(&mut self, name: String) -> Option<old::settings::Value> {
        now::settings::Host::get(self, name).await.map(Into::into)
    }
}

/// `events` has only types, which the plugin receives; it has nothing to call.
impl<L: Library> old::events::Host for Run<L> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_but_playing_reaches_a_0_2_plugin() {
        use now::events::Event as E;
        let track = now::library::Track {
            id: 1,
            title: "Signal".into(),
            artists: vec!["Someone".into()],
            album_id: 2,
            album: None,
            disc_number: 1,
            track_number: Some(1),
            release_date: None,
            isrc: None,
            duration_ms: 1000,
            has_lyrics: false,
            root: 1,
            path: "a.flac".into(),
        };
        let play = now::events::Play {
            track: track.clone(),
            started_at: 5,
            listen_time_ms: 900,
            end: now::events::PlayEnd::Finished,
        };
        let Some(old::events::Event::Played(plays)) = event(&E::Played(vec![play])) else {
            panic!("plays reach a 0.2 plugin");
        };
        assert_eq!((plays[0].track.title.as_str(), plays[0].listen_time_ms), ("Signal", 900));
        assert!(matches!(event(&E::Run), Some(old::events::Event::Run)));
        let playing = now::events::Playing { track, started_at: 5 };
        assert!(event(&E::Playing(playing)).is_none());
        assert!(event(&E::SearchesForgotten(None)).is_none());
        assert!(permission(now::host::Permission::Playing).is_none());
        assert!(permission(now::host::Permission::Searched).is_none());
    }
}
