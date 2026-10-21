//! A worker's panic is logged once, on its own thread, through the global subscriber. The only
//! test in its binary, since it sets the process's global subscriber.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jewelcase_scanner::Worker;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// A worker whose thread returns logs nothing; one whose thread panics logs an error once.
#[test]
fn a_panicked_worker_is_logged_once() {
    let errors = Arc::new(Errors(AtomicUsize::new(0)));
    tracing::subscriber::set_global_default(errors.clone()).unwrap();
    let logged = || errors.0.load(Ordering::SeqCst);

    drop(Worker::spawn("test-returns", |_| ()).unwrap());
    assert_eq!(logged(), 0, "a worker that returned was logged");
    drop(Worker::spawn("test-panics", |_| panic!("a worker bug")).unwrap());
    assert_eq!(logged(), 1, "the panic was not logged once");
}

/// Counts the error events logged.
struct Errors(AtomicUsize);

impl Subscriber for Errors {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() == Level::ERROR {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
