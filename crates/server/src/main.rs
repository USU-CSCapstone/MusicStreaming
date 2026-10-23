use std::process::ExitCode;

use jewelcase_server::config::Config;
use tracing_subscriber::EnvFilter;

/// Reads the configuration from the environment, starts logging, and runs the server until it
/// stops. Exits with failure, saying why, if the configuration is invalid or the server fails.
#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    tracing_subscriber::fmt().with_env_filter(EnvFilter::new(&config.log)).init();

    match jewelcase_server::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}
