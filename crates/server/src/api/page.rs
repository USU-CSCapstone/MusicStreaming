//! What every browse list shares: its parameters, its response, and its total. The queries are
//! the catalog's (`jewelcase_catalog::page`), which a device runs too.

pub use jewelcase_catalog::page::*;

#[cfg(test)]
mod tests {
    use super::library_number;
    use crate::api::testing::app_with_tracks;

    #[tokio::test]
    async fn a_library_that_is_gone_counts_nothing() {
        let (_temp, db, _app) = app_with_tracks().await;
        let count = |library| db.read(move |conn| library_number(conn, library, "track_count"));
        assert!(count(1).await.unwrap() > 0);
        assert_eq!(count(999).await.unwrap(), 0);
    }
}
