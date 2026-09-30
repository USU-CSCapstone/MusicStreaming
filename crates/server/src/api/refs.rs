//! Small references to other entities, shared by the browse responses.

use serde::{Deserialize, Serialize};

use super::Id;

/// The spec's `ArtistCredit`. Read from [`artists_json!`]'s `[id, name]`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credit {
    pub id: Id,
    /// `None` for the unknown artist.
    pub name: Option<String>,
}

/// The spec's `TagRef`. Read from [`genres_json!`]'s `[id, name]`.
#[derive(Clone, Serialize, Deserialize)]
pub struct TagRef {
    pub id: Id,
    pub name: String,
}

/// The spec's `ImageRef`.
#[derive(Clone, Serialize)]
pub struct ImageRef {
    id: Id,
    /// A draft field; empty until the image job makes placeholders (`images.placeholder`).
    placeholder: &'static str,
}

impl ImageRef {
    pub fn new(image_id: Option<i64>) -> Option<ImageRef> {
        image_id.map(|id| ImageRef {
            id: Id(id),
            placeholder: "",
        })
    }
}

/// SQL for the artists credited on an `album` or a `track` whose ID is the SQL `$id`, as JSON
/// that reads as `Vec<Credit>`, in credit order. Part of the row, so a page is one query.
macro_rules! artists_json {
    ($owner:literal, $id:literal) => {
        concat!(
            "(SELECT json_group_array(json_array(CAST(credited.id AS TEXT), credited.name) \
             ORDER BY credit.position) FROM ",
            $owner,
            "_artists credit JOIN artists credited ON credited.id = credit.artist_id WHERE credit.",
            $owner,
            "_id = ",
            $id,
            ")"
        )
    };
}
pub(crate) use artists_json;

/// SQL for the genres of an `album`, an `artist`, or a `track` whose ID is the SQL `$id`, as
/// JSON that reads as `Vec<TagRef>`, in name order.
macro_rules! genres_json {
    ($owner:literal, $id:literal) => {
        concat!(
            "(SELECT json_group_array(json_array(CAST(tag.id AS TEXT), tag.name) \
             ORDER BY tag.sort_key, tag.id) FROM ",
            $owner,
            "_tags tagged JOIN tags tag ON tag.id = tagged.tag_id WHERE tagged.",
            $owner,
            "_id = ",
            $id,
            ")"
        )
    };
}
pub(crate) use genres_json;
