//! What ListenBrainz takes (https://listenbrainz.readthedocs.io/en/latest/users/api/core.html):
//! which plays count as listens, which it would refuse, the body that submits them, and the one
//! that says what is playing now.

use serde_json::{Value, json};

pub const VALIDATE_URL: &str = "https://api.listenbrainz.org/1/validate-token";
pub const SUBMIT_URL: &str = "https://api.listenbrainz.org/1/submit-listens";

/// The most listens one submission may carry.
pub const MAX_PER_SUBMISSION: usize = 1000;

/// A play, as much of it as a listen needs.
pub struct Listen<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub album: Option<&'a str>,
    pub isrc: Option<&'a str>,
    pub track_number: Option<u32>,
    /// 0 when the track's length is unknown.
    pub duration_ms: u64,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub listen_time_ms: u64,
}

/// Whether a play counts as a listen: ListenBrainz asks for half the track or four minutes
/// heard, whichever is less. A track of unknown length needs the four minutes.
pub fn counts(duration_ms: u64, listen_time_ms: u64) -> bool {
    let half = if duration_ms == 0 { u64::MAX } else { duration_ms / 2 };
    listen_time_ms >= half.min(4 * 60_000)
}

/// Whether ListenBrainz would take `l`: it refuses a title or artist that is blank or holds a
/// NUL character. An album or ISRC holding one is left out of the listen instead.
pub fn sendable(l: &Listen) -> bool {
    let usable = |s: &str| !s.trim().is_empty() && !s.contains('\0');
    usable(l.title) && usable(&artist_name(l))
}

/// Sends `listens` through `post`, [`MAX_PER_SUBMISSION`] at a time, where `post` submits one
/// batch and answers ListenBrainz's status and reply. One invalid listen makes ListenBrainz
/// refuse its whole batch with a 400, so a refused batch is split in halves and each sent again,
/// until only the listens it refuses are left out: one bad listen costs about two requests per
/// halving, and a batch of n listens that are all bad costs 2n − 1. Answers how many were turned
/// away; any other refusal or failure is an error, so the plays are kept to send again.
pub fn submit<F>(listens: &[Listen], mut post: F) -> Result<usize, String>
where
    F: FnMut(&[Listen]) -> Result<(u16, String), String>,
{
    listens.chunks(MAX_PER_SUBMISSION).map(|batch| send(batch, &mut post)).sum()
}

/// Sends one batch for [`submit`], and each half again if ListenBrainz refuses it. Answers how
/// many listens were turned away.
fn send<F>(batch: &[Listen], post: &mut F) -> Result<usize, String>
where
    F: FnMut(&[Listen]) -> Result<(u16, String), String>,
{
    match post(batch)? {
        (200, _) => Ok(0),
        (400, _) if batch.len() > 1 => {
            let (front, back) = batch.split_at(batch.len() / 2);
            Ok(send(front, post)? + send(back, post)?)
        }
        (400, _) => Ok(1),
        (status, answer) => Err(format!("ListenBrainz answered {status}: {}", answer.trim())),
    }
}

/// What became of the plays of one delivery.
pub struct Delivery {
    /// Every play delivered.
    pub plays: usize,
    /// Too short to count as listens.
    pub short: usize,
    /// With no title or artist ListenBrainz would take ([`sendable`]).
    pub unnamed: usize,
    /// Sent, and turned away.
    pub rejected: usize,
}

impl Delivery {
    /// The run's summary.
    pub fn summary(&self) -> String {
        let sent = self.plays - self.short - self.unnamed - self.rejected;
        let mut s = format!("Sent {sent} listens; {} plays were too short to count", self.short);
        if self.unnamed > 0 {
            s.push_str(&format!(", {} had no usable title or artist", self.unnamed));
        }
        if self.rejected > 0 {
            s.push_str(&format!(", ListenBrainz turned away {}", self.rejected));
        }
        s.push('.');
        s
    }
}

/// The body that submits `listens`: an import, which takes any number up to
/// [`MAX_PER_SUBMISSION`].
pub fn submission(listens: &[Listen]) -> Value {
    let payload: Vec<Value> = listens.iter().map(listen).collect();
    json!({ "listen_type": "import", "payload": payload })
}

/// The body that tells ListenBrainz `l` is playing now. ListenBrainz shows it until the track
/// would have ended, and takes it with no `listened_at`, since it is not yet a listen.
pub fn playing_now(l: &Listen) -> Value {
    json!({ "listen_type": "playing_now", "payload": [{ "track_metadata": metadata(l) }] })
}

fn listen(l: &Listen) -> Value {
    json!({ "listened_at": l.started_at / 1000, "track_metadata": metadata(l) })
}

/// The artists, as the one name ListenBrainz shows.
fn artist_name(l: &Listen) -> String {
    l.artists.join(", ")
}

/// The track's details. ListenBrainz refuses a length of 0, so an unknown one is left out.
fn metadata(l: &Listen) -> Value {
    let mut info = json!({
        "media_player": "Jewelcase",
        "submission_client": "Jewelcase ListenBrainz plugin",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
    });
    if l.duration_ms > 0 {
        info["duration_ms"] = l.duration_ms.into();
    }
    if let Some(isrc) = l.isrc.filter(|i| !i.contains('\0')) {
        info["isrc"] = isrc.into();
    }
    if let Some(n) = l.track_number {
        info["tracknumber"] = n.into();
    }
    if l.artists.len() > 1 {
        info["artist_names"] = l.artists.into();
    }
    let mut metadata = json!({
        "artist_name": artist_name(l),
        "track_name": l.title,
        "additional_info": info,
    });
    if let Some(album) = l.album.filter(|a| !a.is_empty() && !a.contains('\0')) {
        metadata["release_name"] = album.into();
    }
    metadata
}

/// Whether the answer to a token check says it is valid.
pub fn token_valid(status: u16, body: &str) -> bool {
    status == 200
        && serde_json::from_str::<Value>(body).is_ok_and(|v| v["valid"] == Value::Bool(true))
}

#[cfg(test)]
mod tests;
