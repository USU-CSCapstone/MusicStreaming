//! Configuration, read from `JEWELCASE_*` environment variables (requirements/deployment.md §1).
//!
//! Every setting has a working default. Anything invalid, including an unrecognized
//! `JEWELCASE_*` variable, fails startup with every problem listed at once.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

const PREFIX: &str = "JEWELCASE_";

pub const DATA_DIR: &str = "JEWELCASE_DATA_DIR";
pub const MUSIC: &str = "JEWELCASE_MUSIC";
const PORT: &str = "JEWELCASE_PORT";
const BASE_PATH: &str = "JEWELCASE_BASE_PATH";
const LOG: &str = "JEWELCASE_LOG";

const SETTINGS: [&str; 5] = [DATA_DIR, MUSIC, PORT, BASE_PATH, LOG];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The one directory Jewelcase writes to.
    pub data_dir: PathBuf,
    /// The music to create the first library from, if the server has none yet. A stand-in until
    /// libraries can be created through the API.
    pub music: Option<PathBuf>,
    /// The port the server listens on.
    pub port: u16,
    /// The prefix the server is mounted under.
    pub base_path: String,
    /// A `tracing` filter directive, such as `info` or `jewelcase_server=debug`.
    pub log: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            data_dir: PathBuf::from("/data"),
            music: None,
            port: 8080,
            base_path: String::new(),
            log: "info".to_owned(),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_vars(std::env::vars_os())
    }

    pub fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Result<Config, ConfigError> {
        let mut config = Config::default();
        let mut problems = Vec::new();

        for (key, value) in vars {
            let Some(key) = key.to_str().filter(|key| key.starts_with(PREFIX)) else {
                continue;
            };
            if !SETTINGS.contains(&key) {
                problems.push(format!(
                    "{key} is not a Jewelcase setting; the settings are {}",
                    SETTINGS.join(", ")
                ));
                continue;
            }
            let Ok(value) = value.into_string() else {
                problems.push(format!("{key} is not valid UTF-8"));
                continue;
            };
            let result = match key {
                DATA_DIR => parse_dir(&value).map(|dir| config.data_dir = dir),
                MUSIC => parse_dir(&value).map(|dir| config.music = Some(dir)),
                PORT => parse_port(&value).map(|port| config.port = port),
                BASE_PATH => parse_base_path(&value).map(|path| config.base_path = path),
                LOG => parse_log(&value).map(|log| config.log = log),
                _ => unreachable!("every setting is handled"),
            };
            if let Err(expected) = result {
                problems.push(format!("{key} is {value:?}; expected {expected}"));
            }
        }

        if problems.is_empty() {
            Ok(config)
        } else {
            problems.sort();
            Err(ConfigError { problems })
        }
    }
}

fn parse_dir(value: &str) -> Result<PathBuf, &'static str> {
    if value.is_empty() {
        return Err("a directory path");
    }
    Ok(PathBuf::from(value))
}

fn parse_port(value: &str) -> Result<u16, &'static str> {
    match value.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err("a port number from 1 to 65535"),
    }
}

fn parse_base_path(value: &str) -> Result<String, &'static str> {
    const EXPECTED: &str = "a path such as /music: a leading slash, then segments of letters, digits, '-', '.', '_', or '~'";

    let trimmed = value.strip_suffix('/').unwrap_or(value);
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Err(EXPECTED);
    };
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    };
    if rest.split('/').all(valid_segment) {
        Ok(trimmed.to_owned())
    } else {
        Err(EXPECTED)
    }
}

fn parse_log(value: &str) -> Result<String, &'static str> {
    match EnvFilter::try_new(value) {
        Ok(_) => Ok(value.to_owned()),
        Err(_) => {
            Err("a log level (error, warn, info, debug, trace) or a tracing filter directive")
        }
    }
}

#[derive(Debug)]
pub struct ConfigError {
    problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid configuration:")?;
        for problem in &self.problems {
            write!(f, "\n  {problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_vars(
            vars.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        )
    }

    #[test]
    fn defaults_when_nothing_is_set() {
        assert_eq!(parse(&[("PATH", "/usr/bin")]).unwrap(), Config::default());
    }

    #[test]
    fn reads_every_setting() {
        let config = parse(&[
            ("JEWELCASE_DATA_DIR", "/srv/jewelcase"),
            ("JEWELCASE_MUSIC", "/srv/music"),
            ("JEWELCASE_PORT", "4533"),
            ("JEWELCASE_BASE_PATH", "/music"),
            ("JEWELCASE_LOG", "jewelcase_server=debug"),
        ])
        .unwrap();
        assert_eq!(
            config,
            Config {
                data_dir: PathBuf::from("/srv/jewelcase"),
                music: Some(PathBuf::from("/srv/music")),
                port: 4533,
                base_path: "/music".to_owned(),
                log: "jewelcase_server=debug".to_owned(),
            }
        );
    }

    #[test]
    fn base_path_is_normalized() {
        for (input, expected) in [
            ("", ""),
            ("/", ""),
            ("/music/", "/music"),
            ("/a/b.c~d", "/a/b.c~d"),
        ] {
            assert_eq!(parse_base_path(input), Ok(expected.to_owned()), "{input:?}");
        }
    }

    #[test]
    fn base_path_rejects_malformed_paths() {
        for input in [
            "music",
            "//",
            "/a//b",
            "/a b",
            "/../etc",
            "/music?x",
            "/caf\u{e9}",
        ] {
            assert!(parse_base_path(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn port_must_be_in_range() {
        for input in ["0", "65536", "-1", "http", ""] {
            assert!(parse_port(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn reports_every_problem_at_once() {
        let error = parse(&[
            ("JEWELCASE_PORT", "http"),
            ("JEWELCASE_DATADIR", "/data"),
            ("JEWELCASE_LOG", "info,[[["),
        ])
        .unwrap_err();
        let message = error.to_string();
        assert_eq!(error.problems.len(), 3, "{message}");
        assert!(
            message.contains("JEWELCASE_DATADIR is not a Jewelcase setting"),
            "{message}"
        );
        assert!(
            message.contains("JEWELCASE_PORT is \"http\"; expected a port number"),
            "{message}"
        );
        assert!(message.contains("JEWELCASE_LOG is"), "{message}");
    }
}
