//! Spaces: `/space`, `/space/{key}`, and the content of a space.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::access;
use super::error::{WikiError, WikiResult};
use super::render::{Expand, WikiCtx};
use super::service::{self, content_id, find_content};
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content;
use crate::store::groups;
use crate::store::space_permissions;
use crate::store::spaces::{self, Space, SpaceFilter};
use crate::store::users::{self, User};

/// A space the user can read. Others are reported as missing.
pub async fn find_space(state: &AppState, user: &User, key: &str) -> WikiResult<Space> {
    let space = spaces::by_key(&state.db, key)
        .await?
        .ok_or_else(|| WikiError::space_not_found(key))?;
    access::readable(&state.db, user, space.id, || {
        WikiError::space_not_found(key)
    })
    .await?;
    Ok(space)
}

pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
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
        only: access::readable_spaces(&state.db, &user).await?,
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
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &user, &key).await?;
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
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(request): Json<SpaceRequest>,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &user, &key).await?;
    access::require(&state.db, &user, space.id, "administer", "space").await?;
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
            let page = find_content(&state, &user, &id).await?;
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
    let space = find_space(&state, &user, &key).await?;
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
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
) -> WikiResult<(StatusCode, Json<Value>)> {
    let space = find_space(&state, &user, &key).await?;
    access::require(&state.db, &user, space.id, "administer", "space").await?;
    let files = content::attachment_ids_in_space(&state.db, space.id).await?;
    spaces::delete(&state.db, space.id).await?;
    for id in files {
        crate::store::wiki_files::remove_all(&state.data_dir, id).await;
    }
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
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &user, &key).await?;
    Ok(Json(json!({
        "page": space_content(&state, &space, "page", &params).await?,
        "blogpost": space_content(&state, &space, "blogpost", &params).await?,
        "_links": { "base": format!("{}/wiki", state.base_url), "context": "/wiki" },
    })))
}

pub async fn content_of_type(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((key, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &user, &key).await?;
    if kind != "page" && kind != "blogpost" {
        return Err(WikiError::bad_request(format!(
            "Unsupported content type: {kind}"
        )));
    }
    Ok(Json(space_content(&state, &space, &kind, &params).await?))
}

#[derive(Deserialize)]
pub struct PermissionRequest {
    #[serde(default)]
    subject: Value,
    #[serde(default)]
    operation: Value,
}

/// Checks a requested permission and returns its subject type, subject, operation, and target.
async fn permission_parts(
    state: &AppState,
    request: &PermissionRequest,
) -> WikiResult<(String, String, String, String)> {
    let operation = request.operation["key"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let target = request.operation["target"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if !space_permissions::OPERATIONS.contains(&(operation.as_str(), target.as_str())) {
        return Err(WikiError::bad_request(format!(
            "Unsupported operation: {operation} {target}"
        )));
    }
    let kind = request.subject["type"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let identifier = request.subject["identifier"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let subject = match kind.as_str() {
        "user" => {
            users::by_account_id(&state.db, &identifier)
                .await?
                .ok_or_else(|| {
                    WikiError::bad_request(format!("No user with account id {identifier}"))
                })?;
            identifier
        }
        "group" => {
            let group = groups::find(&state.db, &identifier)
                .await?
                .ok_or_else(|| WikiError::bad_request(format!("No group named {identifier}")))?;
            group.name
        }
        "anonymous" => String::new(),
        other => {
            return Err(WikiError::bad_request(format!(
                "Unsupported subject type: {other}"
            )));
        }
    };
    Ok((kind, subject, operation, target))
}

/// `POST /space/{key}/permission`: grants one operation. Space administrators only.
pub async fn add_permission(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(request): Json<PermissionRequest>,
) -> WikiResult<Json<Value>> {
    let space = find_space(&state, &user, &key).await?;
    access::require(&state.db, &user, space.id, "administer", "space").await?;
    let (kind, subject, operation, target) = permission_parts(&state, &request).await?;
    let permission =
        space_permissions::add(&state.db, space.id, &kind, &subject, &operation, &target).await?;
    Ok(Json(json!({
        "id": permission.id,
        "subject": { "type": kind, "identifier": subject },
        "operation": { "key": operation, "target": target },
        "_links": { "self": format!("{}/wiki/rest/api/space/{}/permission/{}", state.base_url, space.key, permission.id) },
    })))
}

/// `DELETE /space/{key}/permission/{id}`. Space administrators only.
pub async fn remove_permission(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((key, id)): Path<(String, i64)>,
) -> WikiResult<StatusCode> {
    let space = find_space(&state, &user, &key).await?;
    access::require(&state.db, &user, space.id, "administer", "space").await?;
    if !space_permissions::remove(&state.db, space.id, id).await? {
        return Err(WikiError::not_found(format!(
            "No permission {id} in space {key}"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}
