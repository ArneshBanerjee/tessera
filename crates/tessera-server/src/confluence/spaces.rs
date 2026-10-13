//! Spaces: `/space`, `/space/{key}`, and the content of a space.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::error::{WikiError, WikiResult};
use super::render::{Expand, WikiCtx};
use super::service::{self, content_id, find_content};
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content;
use crate::store::spaces::{self, Space, SpaceFilter};

pub async fn find_space(state: &AppState, key: &str) -> WikiResult<Space> {
    spaces::by_key(&state.db, key)
        .await?
        .ok_or_else(|| WikiError::space_not_found(key))
}

pub async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let (start, limit) = params.page(25, 500);
    let filter = SpaceFilter {
        keys: params.all("spaceKey"),
        ids: params
            .all("spaceId")
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect(),
        space_type: params.get("type").map(str::to_string),
        status: params.get("status").map(str::to_string),
    };
    let (page, more) = spaces::list(&state.db, &filter, start, limit).await?;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(&state);
    let mut results = Vec::with_capacity(page.len());
    for space in &page {
        results.push(ctx.space(space, &expand).await?);
    }
    Ok(Json(ctx.collection(results, start, limit, "/space", more)))
}

pub async fn get(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &key).await?;
    Ok(Json(
        WikiCtx::new(&state).space(&space, &params.expand()).await?,
    ))
}

#[derive(Deserialize)]
pub struct SpaceRequest {
    key: Option<String>,
    name: Option<String>,
    #[serde(default)]
    description: Value,
    #[serde(rename = "type")]
    space_type: Option<String>,
    status: Option<String>,
    homepage: Option<Value>,
}

/// The plain text of `{"plain": {"value": "..."}}`.
fn description_text(description: &Value) -> Option<String> {
    description["plain"]["value"].as_str().map(str::to_string)
}

/// Creates a space with a home page, as Confluence does.
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SpaceRequest>,
) -> WikiResult<Json<Value>> {
    let space = service::create_space(
        &state,
        &user,
        request.key.as_deref().unwrap_or_default(),
        request.name.as_deref().unwrap_or_default(),
        &description_text(&request.description).unwrap_or_default(),
        request.space_type.as_deref().unwrap_or("global"),
    )
    .await?;
    Ok(Json(
        WikiCtx::new(&state)
            .space(&space, &Expand::default())
            .await?,
    ))
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(key): Path<String>,
    Json(request): Json<SpaceRequest>,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &key).await?;
    let name = request.name.unwrap_or(space.name);
    if name.trim().is_empty() {
        return Err(WikiError::bad_request("Space name cannot be empty"));
    }
    let description = description_text(&request.description).unwrap_or(space.description);
    let status = request.status.unwrap_or(space.status);
    if status != "current" && status != "archived" {
        return Err(WikiError::bad_request(format!(
            "Invalid space status: {status}"
        )));
    }
    let homepage = match request.homepage.as_ref().and_then(|h| content_id(&h["id"])) {
        Some(id) => {
            let page = find_content(&state, &id).await?;
            if page.space_id != space.id || page.content_type != "page" {
                return Err(WikiError::bad_request(
                    "The home page must be a page in this space",
                ));
            }
            Some(page.id)
        }
        None => space.homepage_id,
    };
    spaces::update(&state.db, space.id, &name, &description, &status, homepage).await?;
    let space = find_space(&state, &key).await?;
    Ok(Json(
        WikiCtx::new(&state)
            .space(&space, &Expand::default())
            .await?,
    ))
}

/// Deletes a space and everything in it. Confluence answers with a long task; Tessera has
/// already finished it.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(key): Path<String>,
) -> WikiResult<(StatusCode, Json<Value>)> {
    let space = find_space(&state, &key).await?;
    spaces::delete(&state.db, space.id).await?;
    let task = uuid::Uuid::new_v4().to_string();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": task,
            "links": { "status": format!("/rest/api/longtask/{task}") },
        })),
    ))
}

/// Pages and blog posts of a space. `depth=root` limits pages to the top level.
async fn space_content(
    state: &AppState,
    space: &Space,
    kind: &str,
    params: &Params,
) -> WikiResult<Value> {
    let (start, limit) = params.page(25, 500);
    let root_only = params.get("depth") == Some("root");
    let all: Vec<content::Content> = match (kind, root_only) {
        ("page", true) => content::root_pages(&state.db, space.id).await?,
        _ => {
            let filter = content::ListFilter {
                space_id: Some(space.id),
                content_type: Some(kind.to_string()),
                ..Default::default()
            };
            content::list(&state.db, &filter).await?
        }
    };
    let more = all.len() as i64 > start + limit;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(state);
    let mut results = Vec::new();
    for item in all.iter().skip(start as usize).take(limit as usize) {
        results.push(ctx.content(item, &expand).await?);
    }
    let path = format!("/space/{}/content/{kind}", space.key);
    Ok(ctx.collection(results, start, limit, &path, more))
}

pub async fn content(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &key).await?;
    Ok(Json(json!({
        "page": space_content(&state, &space, "page", &params).await?,
        "blogpost": space_content(&state, &space, "blogpost", &params).await?,
        "_links": { "base": format!("{}/wiki", state.base_url), "context": "/wiki" },
    })))
}

pub async fn content_of_type(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((key, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &key).await?;
    if kind != "page" && kind != "blogpost" {
        return Err(WikiError::bad_request(format!(
            "Unsupported content type: {kind}"
        )));
    }
    Ok(Json(space_content(&state, &space, &kind, &params).await?))
}
