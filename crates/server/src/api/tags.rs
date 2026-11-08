//! Tags: `listTagTypes`, `listTags` and `getTag` (`api/openapi.yaml`). The queries are the
//! catalog's (`jewelcase_catalog::tags`), which a device runs too.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
#[cfg(test)]
use jewelcase_catalog::tags::source;
use jewelcase_catalog::tags::{self, Tag, TagSort, TagType};
use serde::Deserialize;

use super::extract::{Path, Query};
use super::page::{Order, Page, Request};
use super::scope::Scope;
use super::{Id, Problem};
use crate::db::Database;

/// The parameters of `listTags`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    r#type: Option<String>,
    q: Option<String>,
    #[serde(default)]
    sort: TagSort,
    #[serde(default)]
    order: Order,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `listTagTypes`: each type the library has a tag of, with its count of tags and the name a
/// plugin's type is shown by, in key order. A tag goes with the last track that carries it, so
/// every type listed has music behind it.
pub async fn types(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
) -> Result<Json<Vec<TagType>>, Problem> {
    Ok(Json(db.read(move |conn| tags::types(conn, library)).await?))
}

/// `listTags`: the library's tags, of `type` when it names one and with `q` in their names when
/// it holds more than spaces. A type the library has no tag of lists none.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Tag>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let (tag_type, q) = (query.r#type, query.q);
    let page =
        db.read(move |conn| tags::list(conn, library, request, tag_type, q.as_deref())).await?;
    Ok(Json(page))
}

/// `getTag`: the tag, or not found when the library has no such tag.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<Json<Tag>, Problem> {
    db.read(move |conn| tags::find(conn, library, id))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

#[cfg(test)]
mod tests;
