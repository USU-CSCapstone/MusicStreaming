//! What every plugin's requests share, kept for the life of the host rather than one run, and
//! apart from Wasmtime so it is testable: how often each destination may be asked (`limits`),
//! and the replies already fetched (`cache`).

pub mod cache;
pub mod limits;

#[derive(Default)]
pub struct Network {
    pub limits: limits::Limits,
    pub cache: cache::Cache,
}
