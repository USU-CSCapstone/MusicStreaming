//! Page cursors: where the next page of a list starts (`design/database.md` §4).
//!
//! A cursor holds a label naming the list and sort it came from, then the last row's sort
//! values, which the next query binds as parameters in a keyset condition. Each value is a type
//! byte and its bytes, with a length before text and blobs; the whole is URL-safe base64. That is
//! opaque to clients but not secret, since it holds only values the caller was just sent. It is
//! not signed either: a forged cursor can only choose where a page starts, and the query still
//! runs within the caller's scope.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use rusqlite::types::Value;

use super::Problem;

const NULL: u8 = 0;
const INTEGER: u8 = 1;
const REAL: u8 = 2;
const TEXT: u8 = 3;
const BLOB: u8 = 4;

/// Makes the cursor for the page after the row whose sort values are `after`. `label` names
/// the list and sort, such as `albums.name.asc`, so a cursor is only accepted where it came from.
pub fn encode(label: &str, after: &[Value]) -> String {
    let mut bytes = Vec::new();
    put_bytes(&mut bytes, label.as_bytes());
    for value in after {
        match value {
            Value::Null => bytes.push(NULL),
            Value::Integer(i) => {
                bytes.push(INTEGER);
                bytes.extend(i.to_be_bytes());
            }
            Value::Real(r) => {
                bytes.push(REAL);
                bytes.extend(r.to_be_bytes());
            }
            Value::Text(t) => {
                bytes.push(TEXT);
                put_bytes(&mut bytes, t.as_bytes());
            }
            Value::Blob(b) => {
                bytes.push(BLOB);
                put_bytes(&mut bytes, b);
            }
        }
    }
    BASE64URL.encode(bytes)
}

/// Reads a cursor made by [`encode`] with the same `label` and `columns` sort values. Anything
/// else is `422 validation_failed`.
pub fn decode(text: &str, label: &str, columns: usize) -> Result<Vec<Value>, Problem> {
    read(text, label, columns).ok_or_else(|| Problem::invalid("cursor is not valid for this list"))
}

fn read(text: &str, label: &str, columns: usize) -> Option<Vec<Value>> {
    // No length limit is needed here: the server has already read the whole request, and
    // decoding allocates less than it.
    let bytes = BASE64URL.decode(text).ok()?;
    let mut reader = Reader(&bytes);
    if reader.bytes()? != label.as_bytes() {
        return None;
    }
    let mut values = Vec::with_capacity(columns);
    for _ in 0..columns {
        values.push(match reader.byte()? {
            NULL => Value::Null,
            INTEGER => Value::Integer(i64::from_be_bytes(reader.eight()?)),
            REAL => Value::Real(f64::from_be_bytes(reader.eight()?)),
            TEXT => Value::Text(String::from_utf8(reader.bytes()?.to_vec()).ok()?),
            BLOB => Value::Blob(reader.bytes()?.to_vec()),
            _ => return None,
        });
    }
    reader.0.is_empty().then_some(values)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("a sort value is under 4 GiB");
    out.extend(len.to_be_bytes());
    out.extend(bytes);
}

/// Reads a cursor's bytes from the front. Every read is checked against what is left, so a
/// forged length can never read or allocate past the input.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn eight(&mut self) -> Option<[u8; 8]> {
        self.take(8)?.try_into().ok()
    }

    /// A length-prefixed run of bytes.
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = u32::from_be_bytes(self.take(4)?.try_into().ok()?);
        self.take(usize::try_from(len).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: &str = "albums.name.asc";

    fn row() -> Vec<Value> {
        vec![
            Value::Blob(b"beatles".to_vec()),
            Value::Text("1969-09-26".to_owned()),
            Value::Null,
            Value::Real(-8.5),
            Value::Integer(6_679_900_963_316_241_941),
        ]
    }

    fn code(problem: Problem) -> serde_json::Value {
        serde_json::to_value(problem).unwrap()["code"].clone()
    }

    #[test]
    fn round_trips_every_sort_value() {
        let cursor = encode(LABEL, &row());
        let url_safe = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'_';
        assert!(cursor.bytes().all(url_safe), "{cursor}");
        assert_eq!(decode(&cursor, LABEL, 5).unwrap(), row());
    }

    #[test]
    fn stays_compact_for_long_titles() {
        let title =
            "symphony no. 9 in d minor, op. 125 'choral': iv. presto — allegro assai".repeat(4);
        let cursor = encode(
            LABEL,
            &[Value::Blob(title.clone().into_bytes()), Value::Integer(1)],
        );
        assert!(
            cursor.len() < title.len() * 4 / 3 + 100,
            "{} chars",
            cursor.len()
        );
    }

    #[test]
    fn is_only_accepted_where_it_came_from() {
        let cursor = encode(LABEL, &row());
        for (label, columns) in [
            ("albums.name.desc", 5),
            ("tracks.name.asc", 5),
            (LABEL, 4),
            (LABEL, 6),
        ] {
            let problem = decode(&cursor, label, columns).unwrap_err();
            assert_eq!(code(problem), "validation_failed", "{label} {columns}");
        }
    }

    #[test]
    fn rejects_anything_malformed() {
        let bytes = |cursor: String| BASE64URL.decode(cursor).unwrap();
        let valid = bytes(encode(LABEL, &row()));
        // A cursor with the right label and then `value`, one bad value.
        let label_then =
            |value: &[u8]| BASE64URL.encode([&bytes(encode(LABEL, &[])), value].concat());
        let cases = [
            (String::new(), 5),
            ("abc".to_owned(), 5),
            ("é".to_owned(), 5),
            (encode(LABEL, &row()) + "=", 5), // padded
            ("ab+/".to_owned(), 5),           // not the URL-safe alphabet
            (BASE64URL.encode(&valid[..valid.len() - 1]), 5), // truncated
            (BASE64URL.encode([&valid[..], &[0]].concat()), 5), // trailing bytes
            (label_then(&[9]), 1),            // unknown type
            (label_then(&[3, 0, 0, 0, 1, 0xff]), 1), // text that is not UTF-8
            (label_then(&[4, 0xff, 0xff, 0xff, 0xff]), 1), // a length past the end
        ];
        for (text, columns) in cases {
            let problem = decode(&text, LABEL, columns).unwrap_err();
            assert_eq!(code(problem), "validation_failed", "{text:?}");
        }
    }
}
