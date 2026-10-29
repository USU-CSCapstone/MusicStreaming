//! What the browse responses share: small references to other entities, and the fields every
//! entity reads alike.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Deserializer, Serialize};

use super::Id;

/// The spec's `ArtistRef`. Read from [`artists_json!`]'s `[id, name]`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credit {
    pub id: Id,
    /// `None` for the unknown artist.
    pub name: Option<String>,
}

/// The spec's `TagRef`.
#[derive(Clone, Serialize)]
pub struct TagRef {
    pub id: Id,
    pub name: String,
}

/// The spec's `TagsByType`: an entity's tags by type key, each type's in name order, and a type
/// it has none of left out. Read from [`tags_json!`]'s `[type, id, name]`s.
#[derive(Clone, Default, Serialize)]
#[serde(transparent)]
pub struct TagsByType(BTreeMap<String, Vec<TagRef>>);

impl<'de> Deserialize<'de> for TagsByType {
    /// The tags of an array of `[type, id, name]`, in name order within each type.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<TagsByType, D::Error> {
        let mut tags = BTreeMap::<String, Vec<TagRef>>::new();
        for (tag_type, id, name) in Vec::<(String, Id, String)>::deserialize(deserializer)? {
            tags.entry(tag_type).or_default().push(TagRef { id, name });
        }
        Ok(TagsByType(tags))
    }
}

/// The spec's `ImageRef`.
#[derive(Clone, Serialize)]
pub struct ImageRef {
    id: Id,
    /// The image's ThumbHash in base64, from which a client draws a blurred stand-in before the
    /// image arrives (`requirements/offline.md` §1.1). `None`, sent as `null`, until the
    /// placeholder job has made one, and for an image it could not decode.
    placeholder: Option<String>,
}

impl ImageRef {
    /// The image with this ID, if there is one, and its [`placeholder!`] column, which is NULL
    /// before the placeholder job has run and empty when it could not decode the image.
    pub fn new(image_id: Option<i64>, placeholder: Option<Vec<u8>>) -> Option<ImageRef> {
        image_id.map(|id| ImageRef {
            id: Id(id),
            placeholder: placeholder
                .filter(|hash| !hash.is_empty())
                .map(|hash| BASE64.encode(hash)),
        })
    }
}

/// The spec's `AlbumRef`.
#[derive(Clone, Serialize)]
pub struct AlbumRef {
    pub id: Id,
    pub title: Option<String>,
    pub artists: Vec<Credit>,
    pub image: Option<ImageRef>,
}

/// The spec's `Availability` of a track, or of an album with any track `available`.
pub fn availability(available: bool) -> &'static str {
    if available { "available" } else { "missing" }
}

/// SQL for the placeholder of the image whose ID is the SQL `$image_id`, for [`ImageRef::new`].
macro_rules! placeholder {
    ($image_id:literal) => {
        concat!("(SELECT placeholder FROM images WHERE images.id = ", $image_id, ")")
    };
}
pub(crate) use placeholder;

/// SQL for a JSON array of the SQL `$value`, read from each artist `credited` on an `album` or a
/// `track` whose ID is the SQL `$id`, in credit order. Two such arrays for the same owner line up
/// by position.
macro_rules! credits_json {
    ($value:literal, $owner:literal, $id:literal) => {
        concat!(
            "(SELECT json_group_array(",
            $value,
            " ORDER BY credit.position) FROM ",
            $owner,
            "_artists credit JOIN artists credited ON credited.id = credit.artist_id WHERE credit.",
            $owner,
            "_id = ",
            $id,
            ")"
        )
    };
}
pub(crate) use credits_json;

/// SQL for the artists credited on an `album` or a `track` whose ID is the SQL `$id`, as JSON
/// that reads as `Vec<Credit>`, in credit order. Part of the row, so a page is one query.
macro_rules! artists_json {
    ($owner:literal, $id:literal) => {
        $crate::api::refs::credits_json!(
            "json_array(CAST(credited.id AS TEXT), credited.name)",
            $owner,
            $id
        )
    };
}
pub(crate) use artists_json;

/// SQL for the tags of an `album`, an `artist`, or a `track` whose ID is the SQL `$id`, as JSON
/// that reads as [`TagsByType`]: `[type, id, name]`s in type, then name order. `CROSS JOIN`
/// keeps the entity's own links the outer loop, so statistics showing few tags never have every
/// row scan the library's tags.
macro_rules! tags_json {
    ($owner:literal, $id:literal) => {
        concat!(
            "(SELECT json_group_array(json_array(tag.type, CAST(tag.id AS TEXT), tag.name) \
             ORDER BY tag.type, tag.sort_key, tag.id) FROM ",
            $owner,
            "_tags tagged CROSS JOIN tags tag ON tag.id = tagged.tag_id WHERE tagged.",
            $owner,
            "_id = ",
            $id,
            ")"
        )
    };
}
pub(crate) use tags_json;
