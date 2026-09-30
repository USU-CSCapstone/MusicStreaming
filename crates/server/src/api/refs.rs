//! Small references to other entities, shared by the browse responses.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
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
    /// The image's ThumbHash in base64, from which a client draws a blurred stand-in before the
    /// image arrives (`requirements/offline.md` §1.1). Empty until the placeholder job has made
    /// one, and for an image it could not decode.
    placeholder: String,
}

impl ImageRef {
    /// The image with this ID, if there is one, and its [`placeholder!`] column.
    pub fn new(image_id: Option<i64>, placeholder: Option<Vec<u8>>) -> Option<ImageRef> {
        image_id.map(|id| ImageRef {
            id: Id(id),
            placeholder: BASE64.encode(placeholder.unwrap_or_default()),
        })
    }
}

/// SQL for the placeholder of the image whose ID is the SQL `$image_id`, for [`ImageRef::new`].
macro_rules! placeholder {
    ($image_id:literal) => {
        concat!("(SELECT placeholder FROM images WHERE images.id = ", $image_id, ")")
    };
}
pub(crate) use placeholder;

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
