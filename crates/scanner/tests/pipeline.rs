//! The store conformance suite against the in-memory store.

use jewelcase_scanner::MemoryStore;

jewelcase_scanner::store_suite!(|_root| MemoryStore::new());
