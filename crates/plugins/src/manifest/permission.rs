//! What a plugin can ask for, and how the admin sees it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a plugin can ask for (`requirements/plugins.md` §4.1): what it may reach, and the hooks
/// that run it, which are approved the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    LibraryRead,
    /// Creating files that do not exist yet.
    LibraryAdd,
    /// Replacing, renaming, and deleting files that do.
    LibraryChange,
    Network,
    ListeningActivity,
    /// A hook: run when tracks are added, changed, or removed. Needs library-read too.
    TracksChanged,
    /// A hook: run when a scan of the library finishes. Needs library-read too.
    ScanFinished,
    /// A hook: run at the interval its request names, in every library it is enabled in.
    Schedule,
    /// A hook: run when a user who connected it finishes playing something. Needs
    /// listening-activity, library-read where it was played, and personal settings to connect.
    Played,
    /// A hook: run when a user who connected it starts playing something. Needs what `Played`
    /// does.
    Playing,
    /// The searches of users who turned on sharing them with it.
    SearchActivity,
    /// A hook: run when a user who shares their searches settles on one, or asks for some to be
    /// forgotten. Needs search-activity, library-read, and personal settings to connect.
    Searched,
    /// A hook: run when albums are added, changed, or removed. Needs library-read too.
    AlbumsChanged,
    /// A hook: run when artists are added, changed, or removed. Needs library-read too.
    ArtistsChanged,
    /// Its rate limits that say `"per": "host"` are kept to by every plugin's requests to that
    /// host, so it can slow other plugins. Without it, they count its own.
    HostRateLimits,
}

impl Permission {
    /// Every permission, each once.
    #[cfg(test)]
    pub(crate) const ALL: [Permission; 15] = [
        Self::LibraryRead,
        Self::LibraryAdd,
        Self::LibraryChange,
        Self::Network,
        Self::ListeningActivity,
        Self::TracksChanged,
        Self::ScanFinished,
        Self::Schedule,
        Self::Played,
        Self::Playing,
        Self::SearchActivity,
        Self::Searched,
        Self::AlbumsChanged,
        Self::ArtistsChanged,
        Self::HostRateLimits,
    ];

    /// Its name in manifests, the API and the database, such as `libraryRead`. It is the name its
    /// serde derive reads and writes, so the names a manifest may use and the names stored and
    /// shown come from one place and cannot disagree. Its rate-limit `per` values and the settings
    /// kinds are read the same way.
    pub fn name(self) -> String {
        match serde_json::to_value(self) {
            Ok(Value::String(name)) => name,
            _ => unreachable!("a unit variant serializes as its name"),
        }
    }

    /// The permission named `name` in manifests and the API, if there is one.
    pub fn from_name(name: &str) -> Option<Permission> {
        serde_json::from_value(name.into()).ok()
    }

    /// Granted per library, rather than once for the plugin (`requirements/plugins.md` §4.1).
    pub fn per_library(self) -> bool {
        matches!(
            self,
            Self::LibraryRead
                | Self::LibraryAdd
                | Self::LibraryChange
                | Self::TracksChanged
                | Self::ScanFinished
                | Self::AlbumsChanged
                | Self::ArtistsChanged
        )
    }

    /// How the admin saw it when granting it.
    pub fn title(self) -> &'static str {
        match self {
            Self::LibraryRead => "Read the library",
            Self::LibraryAdd => "Add files to the library",
            Self::LibraryChange => "Change or delete files in the library",
            Self::Network => "Network access",
            Self::ListeningActivity => "Listening activity",
            Self::TracksChanged => "Run when tracks change",
            Self::ScanFinished => "Run when a scan finishes",
            Self::Schedule => "Run on a schedule",
            Self::Played => "Run when a connected user plays something",
            Self::Playing => "Run when a connected user starts playing something",
            Self::SearchActivity => "Search activity",
            Self::Searched => "Run when a user who shares their searches settles on one",
            Self::AlbumsChanged => "Run when albums change",
            Self::ArtistsChanged => "Run when artists change",
            Self::HostRateLimits => "Limit a service for every plugin",
        }
    }
}
