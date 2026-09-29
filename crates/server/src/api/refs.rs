//! Small references to other entities, shared by the browse responses.

use serde::Serialize;

use super::Id;

/// The spec's `ArtistCredit`.
#[derive(Serialize)]
pub struct Credit {
    pub id: Id,
    /// `None` for the unknown artist.
    pub name: Option<String>,
}

/// The spec's `TagRef`.
#[derive(Serialize)]
pub struct TagRef {
    pub id: Id,
    pub name: String,
}

/// The spec's `ImageRef`.
#[derive(Serialize)]
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
