//! A scrobbler: sends each play a connected user finishes to their own ListenBrainz account,
//! once enough of it was heard to count as a listen, and shows what they are playing now.

pub mod listens;

#[cfg(target_arch = "wasm32")]
mod plugin {
    use crate::listens::{self, Listen};

    wit_bindgen::generate!({ path: "../../crates/plugins/wit", world: "plugin" });

    use jewelcase::plugin::events::{Play, Playing};
    use jewelcase::plugin::library::Track;
    use jewelcase::plugin::{http, settings};

    struct Plugin;

    /// The connected user's token, which `settings.get` answers with in their runs.
    fn token() -> Option<String> {
        match settings::get("token") {
            Some(settings::Value::Text(token)) if !token.is_empty() => Some(token),
            _ => None,
        }
    }

    fn send(
        method: http::Method,
        url: &str,
        token: &str,
        body: Vec<u8>,
    ) -> Result<(u16, String), String> {
        let request = http::Request {
            method,
            url: url.to_owned(),
            headers: vec![
                ("authorization".into(), format!("Token {token}")),
                ("content-type".into(), "application/json".into()),
            ],
            body,
        };
        http::send(&request).map(|r| (r.status, String::from_utf8_lossy(&r.body).into_owned()))
    }

    fn listen(track: &Track, started_at: u64, listen_time_ms: u64) -> Listen<'_> {
        Listen {
            title: &track.title,
            artists: &track.artists,
            album: track.album.as_deref(),
            isrc: track.isrc.as_deref(),
            track_number: track.track_number,
            duration_ms: track.duration_ms,
            started_at,
            listen_time_ms,
        }
    }

    fn submit(plays: &[Play], token: &str) -> Result<String, String> {
        let listens: Vec<Listen> = plays
            .iter()
            .filter(|p| listens::counts(p.track.duration_ms, p.listen_time_ms))
            .map(|p| listen(&p.track, p.started_at, p.listen_time_ms))
            .collect();
        let short = plays.len() - listens.len();
        for batch in listens.chunks(listens::MAX_PER_SUBMISSION) {
            let body = listens::submission(batch).to_string().into_bytes();
            let (status, answer) = send(http::Method::Post, listens::SUBMIT_URL, token, body)?;
            if status != 200 {
                // Failing keeps the plays, so they are sent again once it works.
                return Err(format!("ListenBrainz answered {status}: {}", answer.trim()));
            }
        }
        Ok(format!("Sent {} listens; {short} plays were too short to count.", listens.len()))
    }

    /// Tells ListenBrainz what is playing now. A failure is reported and not made up later:
    /// the host never delivers a start late.
    fn announce(now: &Playing, token: &str) -> Result<String, String> {
        let body = listens::playing_now(&listen(&now.track, now.started_at, 0));
        let body = body.to_string().into_bytes();
        let (status, answer) = send(http::Method::Post, listens::SUBMIT_URL, token, body)?;
        if status != 200 {
            return Err(format!("ListenBrainz answered {status}: {}", answer.trim()));
        }
        Ok(format!("Showing {} as playing now.", now.track.title))
    }

    impl Guest for Plugin {
        fn handle(event: Event) -> Result<String, String> {
            match event {
                // A user connecting is checked against ListenBrainz itself, so a mistyped token
                // is caught as it is entered. There is nothing else to check.
                Event::CheckSettings => {
                    let Some(token) = token() else { return Ok("Nothing to check.".into()) };
                    let (status, body) =
                        send(http::Method::Get, listens::VALIDATE_URL, &token, Vec::new())?;
                    if listens::token_valid(status, &body) {
                        Ok("ListenBrainz accepted the token.".into())
                    } else {
                        Err("ListenBrainz did not accept this token. Copy it again from listenbrainz.org/settings.".into())
                    }
                }
                Event::Played(plays) => match token() {
                    Some(token) => submit(&plays, &token),
                    None => Err("no ListenBrainz token is set for this user".into()),
                },
                Event::Playing(now) => match token() {
                    Some(token) => announce(&now, &token),
                    None => Err("no ListenBrainz token is set for this user".into()),
                },
                _ => Ok("Nothing to do.".into()),
            }
        }
    }

    export!(Plugin);
}
