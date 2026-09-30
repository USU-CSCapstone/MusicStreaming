//! Fetching a plugin file from a link the admin pasted (`requirements/plugins.md` §5).

use std::time::Duration;

use crate::manifest::InvalidPlugin;

/// The largest plugin file accepted, uploaded or downloaded.
pub const MAX_SIZE: usize = 50 << 20;

const TIMEOUT: Duration = Duration::from_secs(30);

/// The file at `url`, over http(s), up to [`MAX_SIZE`].
pub async fn download(url: &str) -> Result<Vec<u8>, InvalidPlugin> {
    let invalid = |message: String| InvalidPlugin(message);
    let url = reqwest::Url::parse(url).map_err(|_| invalid("That is not a URL.".into()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(invalid("Plugins are fetched over http or https.".into()));
    }
    let failed =
        |e: reqwest::Error| invalid(format!("Could not download it: {}.", e.without_url()));
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(failed)?;
    let mut res = client.get(url).send().await.map_err(failed)?;
    if !res.status().is_success() {
        return Err(invalid(format!("Could not download it: HTTP {}.", res.status().as_u16())));
    }
    let too_big = || invalid("The file is over 50 MB.".into());
    if res.content_length().is_some_and(|len| len > MAX_SIZE as u64) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(failed)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_SIZE {
            return Err(too_big());
        }
    }
    Ok(bytes)
}
