//! Which destinations a plugin may reach over the network, kept apart from Wasmtime so it is
//! testable.

/// Whether `url` may be fetched, given the destinations the manifest declared. Only
/// http(s), and only to a listed host (or any, for `*`); subdomains must be listed too.
pub fn allowed(url: &str, destinations: &[String]) -> Result<(), String> {
    let url = reqwest::Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(format!("{} is not allowed; only http and https", url.scheme()));
    }
    // Parsed, so userinfo and ports are already apart from the host, which is lowercase.
    let host = url.host_str().ok_or("the URL has no host")?;
    if destinations.iter().any(|d| d == "*" || d.eq_ignore_ascii_case(host)) {
        Ok(())
    } else {
        Err(format!("{host} is not one of this plugin's approved destinations"))
    }
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
        assert!(allowed("https://LRCLIB.net:443/api", &only).is_ok());
        assert!(allowed("https://evil.example/api", &only).is_err());
        assert!(allowed("https://lrclib.net.evil.example/", &only).is_err());
        assert!(allowed("https://api.lrclib.net/", &only).is_err());
        assert!(allowed("https://lrclib.net@evil.example/", &only).is_err());
        assert!(allowed("file:///etc/passwd", &only).is_err());
        assert!(allowed("https://anything.example/", &d(&["*"])).is_ok());
    }
}
