//! Which destinations a plugin may reach over the network, kept apart from Wasmtime so it is
//! testable.

/// Whether `url` may be fetched, given the destinations the manifest declared. Only
/// http(s), and only to a listed host (or any, for `*`); subdomains must be listed too.
/// Answers the URL as parsed, so every spelling of it is one, with its host lowercase.
pub fn allowed(url: &str, destinations: &[String]) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(format!("{} is not allowed; only http and https", parsed.scheme()));
    }
    // Parsed, so userinfo and ports are already apart from the host, which is lowercase.
    let host = parsed.host_str().ok_or("the URL has no host")?;
    // `nas.` and `nas` can be different hosts, and `a.com.` would escape `a.com`'s rate limits,
    // so a host is named without the dot.
    if host.ends_with('.') {
        return Err(format!("{host} ends with a dot; name the host without it"));
    }
    if reaches(host, destinations) {
        Ok(parsed)
    } else {
        Err(format!("{host} is not one of this plugin's approved destinations"))
    }
}

/// Whether `destinations` let a plugin reach `host`: one names it, or is `*`.
pub fn reaches(host: &str, destinations: &[String]) -> bool {
    destinations.iter().any(|d| d == "*" || d.eq_ignore_ascii_case(host))
}

/// The host `url` names, lowercase as the parser gives an http(s) host, or nothing if it is
/// not a URL.
pub fn host_of(url: &str) -> String {
    let url = reqwest::Url::parse(url).ok();
    url.as_ref().and_then(|url| url.host_str()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn destinations_are_exact() {
        let only = d(&["lrclib.net"]);
        assert!(allowed("https://lrclib.net/api/get?x=1", &only).is_ok());
        let parsed = allowed("https://LRCLIB.net:443/api", &only).map(String::from);
        assert_eq!(parsed, Ok("https://lrclib.net/api".into()));
        // A trailing dot is refused rather than read as the same host.
        assert!(allowed("https://lrclib.net./api", &only).is_err());
        assert!(allowed("https://lrclib.net./api", &d(&["*"])).is_err());
        assert!(allowed("https://evil.example/api", &only).is_err());
        assert!(allowed("https://lrclib.net.evil.example/", &only).is_err());
        assert!(allowed("https://api.lrclib.net/", &only).is_err());
        assert!(allowed("https://lrclib.net@evil.example/", &only).is_err());
        assert!(allowed("file:///etc/passwd", &only).is_err());
        assert!(allowed("https://anything.example/", &d(&["*"])).is_ok());
    }
}
