//! SQL the responses share for reading rows as the API shows them.

/// SQL for a millisecond timestamp column as an RFC 3339 string, with the milliseconds computed
/// in integer arithmetic so they come out exact.
macro_rules! timestamp {
    ($column:literal) => {
        concat!(
            "strftime('%Y-%m-%dT%H:%M:%S', ",
            $column,
            " / 1000, 'unixepoch') || printf('.%03dZ', ",
            $column,
            " % 1000)"
        )
    };
}
pub(crate) use timestamp;
