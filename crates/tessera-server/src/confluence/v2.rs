//! The Confluence v2 REST API, under `/wiki/api/v2`.
//!
//! v2 differs from v1 in shape, not in rules: ids are strings, related objects are ids instead
//! of nested objects, bodies are asked for with `body-format`, lists page with an opaque
//! `cursor`, and errors are `{"errors": [{"status", "code", "title", "detail"}]}`.

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::error::WikiError;
use super::service::{self, NewPage, Parent, Save, find_content, find_current};
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content::{self, Content, ListFilter, Version};
use crate::store::labels;
use crate::store::spaces::{self, Space, SpaceFilter};
use crate::store::users::{self, User};
use crate::time::format_iso;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/spaces", get(list_spaces).post(create_space))
        .route("/spaces/{id}", get(get_space))
        .route("/spaces/{id}/pages", get(space_pages))
        .route("/spaces/{id}/blogposts", get(space_blogposts))
        .route("/pages", get(list_pages).post(create_page))
        .route(
            "/pages/{id}",
            get(get_content).put(update_content).delete(delete_content),
        )
        .route("/pages/{id}/title", put(update_title))
        .route("/pages/{id}/children", get(children))
        .route("/pages/{id}/direct-children", get(children))
        .route("/pages/{id}/descendants", get(descendants))
        .route("/pages/{id}/ancestors", get(ancestors))
        .route("/pages/{id}/versions", get(versions))
        .route("/pages/{id}/versions/{number}", get(version))
        .route("/pages/{id}/labels", get(content_labels))
        .route("/pages/{id}/footer-comments", get(footer_comments_of))
        .route("/blogposts", get(list_blogposts).post(create_blogpost))
        .route(
            "/blogposts/{id}",
            get(get_content).put(update_content).delete(delete_content),
        )
        .route("/blogposts/{id}/versions", get(versions))
        .route("/blogposts/{id}/versions/{number}", get(version))
        .route("/blogposts/{id}/labels", get(content_labels))
        .route("/blogposts/{id}/footer-comments", get(footer_comments_of))
        .route(
            "/footer-comments",
            get(list_footer_comments).post(create_footer_comment),
        )
        .route(
            "/footer-comments/{id}",
            get(get_footer_comment)
                .put(update_footer_comment)
                .delete(delete_footer_comment),
        )
        .route("/footer-comments/{id}/children", get(comment_replies))
}

/// A v2 error.
pub struct V2Error(WikiError);

type V2Result<T> = Result<T, V2Error>;

impl<E: Into<WikiError>> From<E> for V2Error {
    fn from(error: E) -> Self {
        V2Error(error.into())
    }
}

impl IntoResponse for V2Error {
    fn into_response(self) -> Response {
        let status = self.0.status;
        let code = match status {
            StatusCode::BAD_REQUEST => "INVALID_REQUEST_PARAMETER",
            StatusCode::UNAUTHORIZED => "UNAUTHORIZED",
            StatusCode::FORBIDDEN => "FORBIDDEN",
            StatusCode::NOT_FOUND => "NOT_FOUND",
            StatusCode::CONFLICT => "CONFLICT",
            _ => "INTERNAL_SERVER_ERROR",
        };
        let body = json!({
            "errors": [{
                "status": status.as_u16(),
                "code": code,
                "title": self.0.message,
                "detail": null,
            }],
        });
        (status, axum::Json(body)).into_response()
    }
}

// Paging.

/// An opaque cursor: the offset of the next item.
fn encode_cursor(offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("offset:{offset}"))
}

fn decode_cursor(cursor: Option<&str>) -> V2Result<usize> {
    let Some(cursor) = cursor else { return Ok(0) };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| text.strip_prefix("offset:").and_then(|n| n.parse().ok()))
        .ok_or_else(|| WikiError::bad_request("The cursor is not valid").into())
}

/// One page of `items`, with a `next` link that keeps the request's other parameters.
fn paged(state: &AppState, path: &str, params: &Params, items: Vec<Value>) -> V2Result<Value> {
    let start = decode_cursor(params.get("cursor"))?;
    let limit = params.int("limit").unwrap_or(25).clamp(1, 250) as usize;
    let more = items.len() > start + limit;
    let results: Vec<Value> = items.into_iter().skip(start).take(limit).collect();
    let mut links = json!({ "base": format!("{}/wiki", state.base_url) });
    if more {
        let mut query: Vec<(String, String)> = params
            .pairs()
            .iter()
            .filter(|(k, _)| k != "cursor")
            .cloned()
            .collect();
        query.push(("cursor".into(), encode_cursor(start + limit)));
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(query)
            .finish();
        links["next"] = json!(format!("/wiki/api/v2{path}?{encoded}"));
    }
    Ok(json!({ "results": results, "_links": links }))
}

// Rendering.

/// Caches account ids and space keys while rendering one response.
struct V2Ctx<'a> {
    state: &'a AppState,
    accounts: HashMap<i64, String>,
    space_keys: HashMap<i64, String>,
    /// Loaded the first time a body is drawn for display.
    names: Option<service::Names>,
}

impl<'a> V2Ctx<'a> {
    fn new(state: &'a AppState) -> Self {
        Self {
            state,
            accounts: HashMap::new(),
            space_keys: HashMap::new(),
            names: None,
        }
    }

    async fn account(&mut self, user_id: i64) -> V2Result<String> {
        if !self.accounts.contains_key(&user_id) {
            let account = users::by_id(&self.state.db, user_id)
                .await?
                .map(|u| u.account_id)
                .unwrap_or_default();
            self.accounts.insert(user_id, account);
        }
        Ok(self.accounts[&user_id].clone())
    }

    async fn space_key(&mut self, space_id: i64) -> V2Result<String> {
        if !self.space_keys.contains_key(&space_id) {
            let key = spaces::by_id(&self.state.db, space_id)
                .await?
                .map(|s| s.key)
                .unwrap_or_default();
            self.space_keys.insert(space_id, key);
        }
        Ok(self.space_keys[&space_id].clone())
    }

    async fn space(&mut self, space: &Space, description_format: Option<&str>) -> V2Result<Value> {
        let author = self.account(space.creator_id).await?;
        let mut out = json!({
            "id": space.id.to_string(),
            "key": space.key,
            "name": space.name,
            "type": space.space_type,
            "status": space.status,
            "authorId": author,
            "spaceOwnerId": author,
            "createdAt": format_iso(space.created_at),
            "homepageId": space.homepage_id.map(|id| id.to_string()),
            "currentActiveAlias": space.key,
            "icon": null,
            "_links": { "webui": format!("/spaces/{}", space.key) },
        });
        if let Some(format) = description_format {
            let value = match format {
                "view" => html_escape::encode_text(&space.description).to_string(),
                _ => space.description.clone(),
            };
            out["description"] = json!({ format: { "representation": format, "value": value } });
        }
        Ok(out)
    }

    async fn version(&mut self, version: &Version) -> V2Result<Value> {
        Ok(json!({
            "createdAt": format_iso(version.created_at),
            "message": version.message,
            "number": version.number,
            "minorEdit": version.minor_edit,
            "authorId": self.account(version.author_id).await?,
        }))
    }

    /// The version object of content's current version.
    async fn current_version(&mut self, item: &Content) -> V2Result<Value> {
        Ok(
            match content::version(&self.state.db, item.id, item.version).await? {
                Some(version) => self.version(&version).await?,
                None => Value::Null,
            },
        )
    }

    async fn body(&mut self, item: &Content, format: Option<&str>) -> V2Result<Value> {
        let Some(format) = format else {
            return Ok(json!({}));
        };
        if service::is_view(format) && self.names.is_none() {
            self.names = Some(service::load_names(&self.state.db).await?);
        }
        let empty = service::Names::new();
        let names = self.names.as_ref().unwrap_or(&empty);
        let value = service::body_as(format, &item.body, names)
            .ok_or_else(|| WikiError::bad_request(format!("Unsupported body-format: {format}")))?;
        Ok(json!({ format: { "representation": format, "value": value } }))
    }

    /// A page or blog post.
    async fn content(&mut self, item: &Content, body_format: Option<&str>) -> V2Result<Value> {
        let key = self.space_key(item.space_id).await?;
        let author = self.account(item.creator_id).await?;
        let title = form_urlencoded::byte_serialize(item.title.as_bytes()).collect::<String>();
        let webui = if item.content_type == "blogpost" {
            format!("/spaces/{key}/blog/{}/{title}", item.id)
        } else {
            format!("/spaces/{key}/pages/{}/{title}", item.id)
        };
        let mut out = json!({
            "id": item.id.to_string(),
            "status": item.status,
            "title": item.title,
            "spaceId": item.space_id.to_string(),
            "authorId": author,
            "ownerId": author,
            "lastOwnerId": null,
            "createdAt": format_iso(item.created_at),
            "version": self.current_version(item).await?,
            "body": self.body(item, body_format).await?,
            "_links": {
                "webui": webui,
                "editui": format!("/pages/resumedraft.action?draftId={}", item.id),
                "tinyui": format!("/x/{}", item.id),
            },
        });
        if item.content_type == "page" {
            out["parentId"] = json!(item.parent_id.map(|id| id.to_string()));
            out["parentType"] = json!(item.parent_id.map(|_| "page"));
            out["position"] = json!(item.position);
        }
        Ok(out)
    }

    async fn comment(&mut self, item: &Content, body_format: Option<&str>) -> V2Result<Value> {
        let container = match item.container_id {
            Some(id) => content::by_id(&self.state.db, id).await?,
            None => None,
        };
        let mut out = json!({
            "id": item.id.to_string(),
            "status": item.status,
            "title": item.title,
            "version": self.current_version(item).await?,
            "body": self.body(item, body_format).await?,
            "_links": { "webui": format!("/pages/{}?focusedCommentId={}", item.container_id.unwrap_or_default(), item.id) },
        });
        match &container {
            Some(c) if c.content_type == "blogpost" => out["blogPostId"] = json!(c.id.to_string()),
            Some(c) => out["pageId"] = json!(c.id.to_string()),
            None => {}
        }
        if let Some(parent) = item.parent_id {
            out["parentCommentId"] = json!(parent.to_string());
        }
        Ok(out)
    }
}

/// `body-format`, validated.
fn body_format(params: &Params) -> V2Result<Option<&str>> {
    match params.get("body-format") {
        None | Some("none") => Ok(None),
        Some(
            f @ ("storage"
            | "atlas_doc_format"
            | "view"
            | "export_view"
            | "styled_view"
            | "editor"
            | "anonymous_export_view"),
        ) => Ok(Some(f)),
        Some(other) => {
            Err(WikiError::bad_request(format!("Unsupported body-format: {other}")).into())
        }
    }
}

// Spaces.

async fn find_space_by_id(state: &AppState, id: &str) -> V2Result<Space> {
    let found = match id.parse::<i64>() {
        Ok(number) => spaces::by_id(&state.db, number).await?,
        Err(_) => None,
    };
    found.ok_or_else(|| WikiError::not_found(format!("No space with id {id}")).into())
}

async fn list_spaces(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    let filter = SpaceFilter {
        keys: params.all("keys"),
        ids: params
            .all("ids")
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect(),
        space_type: params.get("type").map(str::to_string),
        status: params.get("status").map(str::to_string),
    };
    let (all, _) = spaces::list(&state.db, &filter, 0, i64::MAX / 2).await?;
    let mut ctx = V2Ctx::new(&state);
    let mut items = Vec::with_capacity(all.len());
    for space in &all {
        items.push(ctx.space(space, params.get("description-format")).await?);
    }
    Ok(Json(paged(&state, "/spaces", &params, items)?))
}

async fn get_space(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let space = find_space_by_id(&state, &id).await?;
    Ok(Json(
        V2Ctx::new(&state)
            .space(&space, params.get("description-format"))
            .await?,
    ))
}

#[derive(Deserialize)]
struct SpaceRequest {
    key: Option<String>,
    name: Option<String>,
    #[serde(default)]
    description: Value,
}

async fn create_space(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SpaceRequest>,
) -> V2Result<Json<Value>> {
    let space = service::create_space(
        &state,
        &user,
        request.key.as_deref().unwrap_or_default(),
        request.name.as_deref().unwrap_or_default(),
        request.description["value"].as_str().unwrap_or_default(),
        "global",
    )
    .await?;
    Ok(Json(V2Ctx::new(&state).space(&space, Some("plain")).await?))
}

/// Pages or blog posts of a space. For pages, `depth=root` keeps only the top level.
async fn space_items(state: &AppState, id: &str, kind: &str, params: &Params) -> V2Result<Value> {
    let space = find_space_by_id(state, id).await?;
    let items = if kind == "page" && params.get("depth") == Some("root") {
        content::root_pages(&state.db, space.id).await?
    } else {
        content::list(
            &state.db,
            &ListFilter {
                space_id: Some(space.id),
                content_type: Some(kind.into()),
                status: params.get("status").map(str::to_string),
                title: params.get("title").map(str::to_string),
                ..Default::default()
            },
        )
        .await?
    };
    let format = body_format(params)?;
    let mut ctx = V2Ctx::new(state);
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        out.push(ctx.content(item, format).await?);
    }
    paged(state, &format!("/spaces/{id}/{kind}s"), params, out)
}

async fn space_pages(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(space_items(&state, &id, "page", &params).await?))
}

async fn space_blogposts(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(space_items(&state, &id, "blogpost", &params).await?))
}

// Pages and blog posts.

/// `GET /pages` and `GET /blogposts`: filtered by id, space, status, and title, then sorted.
async fn list_items(state: &AppState, kind: &str, params: &Params) -> V2Result<Value> {
    let statuses = {
        let given = params.all("status");
        if given.is_empty() {
            vec!["current".to_string()]
        } else {
            given
        }
    };
    let ids: Vec<i64> = params
        .all("id")
        .iter()
        .filter_map(|id| id.parse().ok())
        .collect();
    let space_ids: Vec<i64> = params
        .all("space-id")
        .iter()
        .filter_map(|id| id.parse().ok())
        .collect();
    let mut items = Vec::new();
    for status in &statuses {
        items.extend(
            content::list(
                &state.db,
                &ListFilter {
                    content_type: Some(kind.into()),
                    status: Some(status.clone()),
                    title: params.get("title").map(str::to_string),
                    ..Default::default()
                },
            )
            .await?,
        );
    }
    items.retain(|c| {
        (ids.is_empty() || ids.contains(&c.id))
            && (space_ids.is_empty() || space_ids.contains(&c.space_id))
    });
    match params.get("sort").unwrap_or("id") {
        "id" => items.sort_by_key(|c| c.id),
        "-id" => items.sort_by_key(|c| std::cmp::Reverse(c.id)),
        "created-date" => items.sort_by_key(|c| (c.created_at, c.id)),
        "-created-date" => items.sort_by_key(|c| std::cmp::Reverse((c.created_at, c.id))),
        "modified-date" => items.sort_by_key(|c| (c.updated_at, c.id)),
        "-modified-date" => items.sort_by_key(|c| std::cmp::Reverse((c.updated_at, c.id))),
        "title" => items.sort_by_key(|c| c.title.to_lowercase()),
        "-title" => items.sort_by_key(|c| std::cmp::Reverse(c.title.to_lowercase())),
        other => return Err(WikiError::bad_request(format!("Unsupported sort: {other}")).into()),
    }
    let format = body_format(params)?;
    let mut ctx = V2Ctx::new(state);
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        out.push(ctx.content(item, format).await?);
    }
    paged(state, &format!("/{kind}s"), params, out)
}

async fn list_pages(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(list_items(&state, "page", &params).await?))
}

async fn list_blogposts(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(list_items(&state, "blogpost", &params).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentRequest {
    space_id: Option<Value>,
    status: Option<String>,
    title: Option<String>,
    parent_id: Option<Value>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    version: Value,
}

/// The storage-format body of a request, if given. Bodies come as
/// `{"representation": "storage", "value": ...}`, or keyed by format as
/// `{"storage": {"value": ...}}`.
fn request_body(body: &Value) -> V2Result<Option<String>> {
    let Some(object) = body.as_object().filter(|o| !o.is_empty()) else {
        return Ok(None);
    };
    if let Some(representation) = object.get("representation").and_then(Value::as_str) {
        return Ok(Some(service::body_from(representation, &body["value"])?));
    }
    match object.iter().next() {
        Some((format, inner)) => Ok(Some(service::body_from(format, &inner["value"])?)),
        None => Ok(None),
    }
}

/// The format a request's body came in, to answer in the same format.
fn request_format(body: &Value) -> Option<&str> {
    body["representation"]
        .as_str()
        .or_else(|| {
            body.as_object()
                .and_then(|o| o.keys().next())
                .map(String::as_str)
        })
        .or(Some("storage"))
}

async fn create_item(
    state: &AppState,
    user: &User,
    kind: &str,
    params: &Params,
    request: ContentRequest,
) -> V2Result<Value> {
    let space_id = request
        .space_id
        .as_ref()
        .and_then(service::content_id)
        .ok_or_else(|| WikiError::bad_request("spaceId is required"))?;
    let space = find_space_by_id(state, &space_id).await?;
    // Without a parent, new pages go under the space's home page unless `root-level` is set.
    let parent = match request.parent_id.as_ref().and_then(service::content_id) {
        Some(id) if kind == "page" => Some(service::parent_page(state, &id, space.id).await?),
        _ if kind == "page" && params.get("root-level") != Some("true") => {
            match space.homepage_id {
                Some(home) => content::by_id(&state.db, home)
                    .await?
                    .filter(|h| h.status == "current"),
                None => None,
            }
        }
        _ => None,
    };
    let body = request_body(&request.body)?.unwrap_or_default();
    let created = service::create_page(
        state,
        user,
        NewPage {
            kind,
            status: request.status.as_deref().unwrap_or("current"),
            space: &space,
            title: request.title.as_deref().unwrap_or_default(),
            parent: parent.as_ref(),
            body: &body,
        },
    )
    .await?;
    let format = request_format(&request.body);
    V2Ctx::new(state).content(&created, format).await
}

async fn create_page(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
    Json(request): Json<ContentRequest>,
) -> V2Result<Json<Value>> {
    Ok(Json(
        create_item(&state, &user, "page", &params, request).await?,
    ))
}

async fn create_blogpost(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
    Json(request): Json<ContentRequest>,
) -> V2Result<Json<Value>> {
    Ok(Json(
        create_item(&state, &user, "blogpost", &params, request).await?,
    ))
}

/// A page or blog post (not a comment), found by id.
async fn find_item(state: &AppState, id: &str, current_only: bool) -> V2Result<Content> {
    let found = if current_only {
        find_current(state, id).await?
    } else {
        find_content(state, id).await?
    };
    if found.content_type == "comment" {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_content(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let mut found = find_item(&state, &id, false).await?;
    if found.status == "trashed" && params.get("status") != Some("trashed") {
        return Err(WikiError::content_not_found(&id).into());
    }
    if let Some(number) = params.int("version").filter(|n| *n != found.version) {
        let old = content::version(&state.db, found.id, number)
            .await?
            .ok_or_else(|| WikiError::not_found(format!("No version {number} of content {id}")))?;
        found.title = old.title;
        found.body = old.body;
        found.version = old.number;
        found.status = "historical".into();
    }
    let mut out = V2Ctx::new(&state)
        .content(&found, body_format(&params)?)
        .await?;
    if params.get("include-labels") == Some("true") {
        out["labels"] =
            labels_page(&state, found.id, &params, &format!("/pages/{id}/labels")).await?;
    }
    Ok(Json(out))
}

async fn update_content(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<ContentRequest>,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, false).await?;
    let number = request.version["number"]
        .as_i64()
        .ok_or_else(|| WikiError::bad_request("version.number is required"))?;
    let parent = match request.parent_id.as_ref().and_then(service::content_id) {
        Some(parent)
            if found.content_type == "page"
                && Some(parent.clone()) != found.parent_id.map(|p| p.to_string()) =>
        {
            Parent::Move(Some(
                service::parent_page(&state, &parent, found.space_id).await?,
            ))
        }
        _ => Parent::Keep,
    };
    let save = Save {
        number,
        title: request.title.as_deref(),
        body: request_body(&request.body)?,
        status: request.status.as_deref().unwrap_or("current"),
        parent,
        message: request.version["message"].as_str().unwrap_or_default(),
        minor_edit: request.version["minorEdit"].as_bool().unwrap_or(false),
    };
    let updated = service::save(&state, &user, &found, save).await?;
    let format = request_format(&request.body);
    Ok(Json(V2Ctx::new(&state).content(&updated, format).await?))
}

/// `PUT /pages/{id}/title`: renames a page as a new version.
async fn update_title(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<ContentRequest>,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, true).await?;
    let save = Save {
        number: request.version["number"]
            .as_i64()
            .unwrap_or(found.version + 1),
        title: request.title.as_deref(),
        body: None,
        status: request.status.as_deref().unwrap_or("current"),
        parent: Parent::Keep,
        message: "",
        minor_edit: false,
    };
    let updated = service::save(&state, &user, &found, save).await?;
    Ok(Json(V2Ctx::new(&state).content(&updated, None).await?))
}

/// `DELETE`: moves to the trash; with `purge=true`, deletes trashed content for good.
async fn delete_content(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<StatusCode> {
    let found = find_item(&state, &id, false).await?;
    let purge = params.get("purge") == Some("true");
    if found.status == "trashed" && !purge {
        return Err(WikiError::content_not_found(&id).into());
    }
    service::remove(&state, &found, purge).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn children(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &id, true).await?;
    let items: Vec<Value> = content::children(&state.db, page.id, "page")
        .await?
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "status": c.status,
                "title": c.title,
                "type": "page",
                "spaceId": c.space_id.to_string(),
                "childPosition": c.position,
            })
        })
        .collect();
    Ok(Json(paged(
        &state,
        &format!("/pages/{id}/children"),
        &params,
        items,
    )?))
}

async fn descendants(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &id, true).await?;
    let max_depth = params.int("depth").unwrap_or(5).clamp(1, 5);
    let mut items = Vec::new();
    // Walk depth first, keeping each page's depth below `page`.
    let mut stack: Vec<(Content, i64)> = content::children(&state.db, page.id, "page")
        .await?
        .into_iter()
        .rev()
        .map(|c| (c, 1))
        .collect();
    while let Some((node, depth)) = stack.pop() {
        if depth < max_depth {
            let kids = content::children(&state.db, node.id, "page").await?;
            stack.extend(kids.into_iter().rev().map(|c| (c, depth + 1)));
        }
        items.push(json!({
            "id": node.id.to_string(),
            "status": node.status,
            "title": node.title,
            "type": "page",
            "parentId": node.parent_id.map(|p| p.to_string()),
            "depth": depth,
            "childPosition": node.position,
        }));
    }
    Ok(Json(paged(
        &state,
        &format!("/pages/{id}/descendants"),
        &params,
        items,
    )?))
}

async fn ancestors(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &id, true).await?;
    let results: Vec<Value> = content::ancestors(&state.db, &page)
        .await?
        .iter()
        .map(|a| json!({ "id": a.id.to_string(), "type": "page" }))
        .collect();
    Ok(Json(
        json!({ "results": results, "_links": { "base": format!("{}/wiki", state.base_url) } }),
    ))
}

async fn versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, true).await?;
    let mut ctx = V2Ctx::new(&state);
    let mut items = Vec::new();
    for version in content::versions(&state.db, found.id).await? {
        let mut item = ctx.version(&version).await?;
        let key = if found.content_type == "blogpost" {
            "blogpost"
        } else {
            "page"
        };
        item[key] = json!({ "id": found.id.to_string(), "title": version.title });
        items.push(item);
    }
    Ok(Json(paged(
        &state,
        &format!("/{}s/{id}/versions", found.content_type),
        &params,
        items,
    )?))
}

async fn version(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, number)): Path<(String, i64)>,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, true).await?;
    let version = content::version(&state.db, found.id, number)
        .await?
        .ok_or_else(|| WikiError::not_found(format!("No version {number} of content {id}")))?;
    let mut ctx = V2Ctx::new(&state);
    let mut out = ctx.version(&version).await?;
    if number > 1
        && let Some(previous) = content::version(&state.db, found.id, number - 1).await?
    {
        out["previousVersion"] = ctx.version(&previous).await?;
    }
    if let Some(next) = content::version(&state.db, found.id, number + 1).await? {
        out["nextVersion"] = ctx.version(&next).await?;
    }
    Ok(Json(out))
}

async fn labels_page(
    state: &AppState,
    content_id: i64,
    params: &Params,
    path: &str,
) -> V2Result<Value> {
    let mut list = labels::for_content(&state.db, content_id).await?;
    if let Some(prefix) = params.get("prefix") {
        list.retain(|l| l.prefix == prefix);
    }
    let items = list
        .iter()
        .map(|l| json!({ "id": l.id.to_string(), "name": l.name, "prefix": l.prefix }))
        .collect();
    paged(state, path, params, items)
}

async fn content_labels(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, true).await?;
    let path = format!("/{}s/{id}/labels", found.content_type);
    Ok(Json(labels_page(&state, found.id, &params, &path).await?))
}

// Footer comments.

async fn comments_page(
    state: &AppState,
    items: &[Content],
    params: &Params,
    path: &str,
) -> V2Result<Value> {
    let format = body_format(params)?;
    let mut ctx = V2Ctx::new(state);
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(ctx.comment(item, format).await?);
    }
    paged(state, path, params, out)
}

/// Top-level comments on a page or blog post.
async fn footer_comments_of(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &id, true).await?;
    let top: Vec<Content> = content::comments(&state.db, found.id)
        .await?
        .into_iter()
        .filter(|c| c.parent_id.is_none())
        .collect();
    let path = format!("/{}s/{id}/footer-comments", found.content_type);
    Ok(Json(comments_page(&state, &top, &params, &path).await?))
}

async fn list_footer_comments(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    let all = content::list(
        &state.db,
        &ListFilter {
            content_type: Some("comment".into()),
            ..Default::default()
        },
    )
    .await?;
    Ok(Json(
        comments_page(&state, &all, &params, "/footer-comments").await?,
    ))
}

async fn find_comment(state: &AppState, id: &str) -> V2Result<Content> {
    let found = find_current(state, id).await?;
    if found.content_type != "comment" {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_footer_comment(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &id).await?;
    Ok(Json(
        V2Ctx::new(&state)
            .comment(&found, body_format(&params)?)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentRequest {
    page_id: Option<Value>,
    blog_post_id: Option<Value>,
    parent_comment_id: Option<Value>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    version: Value,
}

async fn create_footer_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<CommentRequest>,
) -> V2Result<Json<Value>> {
    let reply_to = match request
        .parent_comment_id
        .as_ref()
        .and_then(service::content_id)
    {
        Some(id) => Some(find_comment(&state, &id).await?),
        None => None,
    };
    let container_id = match (
        &reply_to,
        request.page_id.as_ref().or(request.blog_post_id.as_ref()),
    ) {
        (Some(parent), _) => parent.container_id.map(|id| id.to_string()),
        (None, Some(id)) => service::content_id(id),
        (None, None) => None,
    }
    .ok_or_else(|| WikiError::bad_request("pageId, blogPostId, or parentCommentId is required"))?;
    let container = find_item(&state, &container_id, true).await?;
    let body = request_body(&request.body)?.unwrap_or_default();
    let created =
        service::create_comment(&state, &user, &container, reply_to.as_ref(), &body).await?;
    let format = request_format(&request.body);
    Ok(Json(V2Ctx::new(&state).comment(&created, format).await?))
}

async fn update_footer_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<CommentRequest>,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &id).await?;
    if found.creator_id != user.id && !user.is_admin {
        return Err(
            WikiError::new(StatusCode::FORBIDDEN, "Only the author can edit a comment").into(),
        );
    }
    let save = Save {
        number: request.version["number"]
            .as_i64()
            .ok_or_else(|| WikiError::bad_request("version.number is required"))?,
        title: None,
        body: request_body(&request.body)?,
        status: "current",
        parent: Parent::Keep,
        message: request.version["message"].as_str().unwrap_or_default(),
        minor_edit: false,
    };
    let updated = service::save(&state, &user, &found, save).await?;
    let format = request_format(&request.body);
    Ok(Json(V2Ctx::new(&state).comment(&updated, format).await?))
}

async fn delete_footer_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> V2Result<StatusCode> {
    let found = find_comment(&state, &id).await?;
    if found.creator_id != user.id && !user.is_admin {
        return Err(WikiError::new(
            StatusCode::FORBIDDEN,
            "Only the author can delete a comment",
        )
        .into());
    }
    service::remove(&state, &found, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn comment_replies(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &id).await?;
    let replies = content::children(&state.db, found.id, "comment").await?;
    Ok(Json(
        comments_page(
            &state,
            &replies,
            &params,
            &format!("/footer-comments/{id}/children"),
        )
        .await?,
    ))
}
