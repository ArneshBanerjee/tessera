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
use crate::store::inline_comments;
use crate::store::labels;
use crate::store::spaces::{self, Space, SpaceFilter};
use crate::store::users::{self, User};
use crate::store::wiki_files;
use crate::time::format_iso;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/spaces", get(list_spaces).post(create_space))
        .route("/spaces/{id}", get(get_space))
        .route("/spaces/{id}/pages", get(space_pages))
        .route("/spaces/{id}/blogposts", get(space_blogposts))
        .route("/spaces/{id}/permissions", get(space_permissions_list))
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
        .route("/pages/{id}/inline-comments", get(inline_comments_of))
        .route("/blogposts/{id}/inline-comments", get(inline_comments_of))
        .route(
            "/inline-comments",
            get(list_inline_comments).post(create_inline_comment),
        )
        .route(
            "/inline-comments/{id}",
            get(get_inline_comment)
                .put(update_inline_comment)
                .delete(delete_footer_comment),
        )
        .route("/inline-comments/{id}/children", get(comment_replies))
        .route("/comments", axum::routing::post(create_any_comment))
        .route(
            "/comments/{id}",
            get(get_any_comment)
                .put(update_any_comment)
                .delete(delete_footer_comment),
        )
        .route("/comments/{id}/children", get(comment_replies))
        .route(
            "/attachments/{id}",
            get(get_attachment).delete(delete_attachment),
        )
        .route("/pages/{id}/attachments", get(attachments_of))
        .route("/blogposts/{id}/attachments", get(attachments_of))
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
        let view = service::View {
            names: self.names.as_ref().unwrap_or(&empty),
            page_id: Some(super::render::images_page(item)),
        };
        let value = service::body_as(format, &item.body, &view)
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

    async fn attachment(&mut self, item: &Content) -> V2Result<Value> {
        let file = wiki_files::find(&self.state.db, item.id, Some(item.version)).await?;
        let (media_type, size, comment) = file
            .map(|f| (f.media_type, f.size, f.comment))
            .unwrap_or_default();
        let container = item.container_id.unwrap_or_default();
        let container_type = match content::by_id(&self.state.db, container).await? {
            Some(c) if c.content_type == "blogpost" => "blogPostId",
            _ => "pageId",
        };
        let name = form_urlencoded::byte_serialize(item.title.as_bytes()).collect::<String>();
        let download = format!(
            "{}?version={}&api=v2",
            service::download_path(container, &item.title),
            item.version
        );
        let webui = format!(
            "/pages/viewpageattachments.action?pageId={container}&preview=%2F{container}%2F{name}"
        );
        let mut out = json!({
            "id": format!("att{}", item.id),
            "status": item.status,
            "title": item.title,
            "createdAt": format_iso(item.created_at),
            "mediaType": media_type,
            "mediaTypeDescription": media_type,
            "comment": comment,
            "fileId": format!("{}-{}", item.id, item.version),
            "fileSize": size,
            "webuiLink": webui,
            "downloadLink": download,
            "version": self.current_version(item).await?,
            "_links": { "webui": webui, "download": download },
        });
        out[container_type] = json!(container.to_string());
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
        if let Some(inline) = inline_comments::get(&self.state.db, item.id).await? {
            let page_body = container
                .as_ref()
                .map(|c| c.body.as_str())
                .unwrap_or_default();
            out["resolutionStatus"] = json!(inline.status(page_body));
            out["properties"] = json!({
                "inlineMarkerRef": inline.marker_ref,
                "inlineOriginalSelection": inline.original_selection,
            });
            if let Some(by) = inline.resolution_by
                && let Some(user) = users::by_id(&self.state.db, by).await?
            {
                out["resolutionLastModifierId"] = json!(user.account_id);
            }
            if let Some(at) = inline.resolution_at {
                out["resolutionLastModifiedAt"] = json!(crate::time::format_iso(at));
            }
        }
        Ok(out)
    }
}

/// Keeps footer comments, or inline ones.
async fn retain_location(state: &AppState, items: &mut Vec<Content>, inline: bool) -> V2Result<()> {
    let mut kept = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        if inline_comments::get(&state.db, item.id).await?.is_some() == inline {
            kept.push(item);
        }
    }
    *items = kept;
    Ok(())
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

/// A space the user can read. Others are reported as missing.
async fn find_space_by_id(state: &AppState, user: &User, id: &str) -> V2Result<Space> {
    let missing = || WikiError::not_found(format!("No space with id {id}"));
    let found = match id.parse::<i64>() {
        Ok(number) => spaces::by_id(&state.db, number).await?,
        Err(_) => None,
    };
    let space = found.ok_or_else(missing)?;
    super::access::readable(&state.db, user, space.id, missing).await?;
    Ok(space)
}

async fn list_spaces(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
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
        only: super::access::readable_spaces(&state.db, &user).await?,
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let space = find_space_by_id(&state, &user, &id).await?;
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
async fn space_items(
    state: &AppState,
    user: &User,
    id: &str,
    kind: &str,
    params: &Params,
) -> V2Result<Value> {
    let space = find_space_by_id(state, user, id).await?;
    let mut items = if kind == "page" && params.get("depth") == Some("root") {
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
    super::access::retain_visible(&state.db, user, &mut items).await?;
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(
        space_items(&state, &user, &id, "page", &params).await?,
    ))
}

async fn space_blogposts(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(
        space_items(&state, &user, &id, "blogpost", &params).await?,
    ))
}

// Pages and blog posts.

/// `GET /pages` and `GET /blogposts`: filtered by id, space, status, and title, then sorted.
async fn list_items(state: &AppState, user: &User, kind: &str, params: &Params) -> V2Result<Value> {
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
    super::access::retain_visible(&state.db, user, &mut items).await?;
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
    CurrentUser(user): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(list_items(&state, &user, "page", &params).await?))
}

async fn list_blogposts(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    Ok(Json(list_items(&state, &user, "blogpost", &params).await?))
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
    let space = find_space_by_id(state, user, &space_id).await?;
    // Without a parent, new pages go under the space's home page unless `root-level` is set.
    let parent = match request.parent_id.as_ref().and_then(service::content_id) {
        Some(id) if kind == "page" => Some(service::parent_page(state, user, &id, space.id).await?),
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
async fn find_item(
    state: &AppState,
    user: &User,
    id: &str,
    current_only: bool,
) -> V2Result<Content> {
    let found = if current_only {
        find_current(state, user, id).await?
    } else {
        find_content(state, user, id).await?
    };
    if found.content_type != "page" && found.content_type != "blogpost" {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_content(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let mut found = find_item(&state, &user, &id, false).await?;
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
    let found = find_item(&state, &user, &id, false).await?;
    let number = request.version["number"]
        .as_i64()
        .ok_or_else(|| WikiError::bad_request("version.number is required"))?;
    let parent = match request.parent_id.as_ref().and_then(service::content_id) {
        Some(parent)
            if found.content_type == "page"
                && Some(parent.clone()) != found.parent_id.map(|p| p.to_string()) =>
        {
            Parent::Move(Some(
                service::parent_page(&state, &user, &parent, found.space_id).await?,
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
    let found = find_item(&state, &user, &id, true).await?;
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<StatusCode> {
    let found = find_item(&state, &user, &id, false).await?;
    let purge = params.get("purge") == Some("true");
    if found.status == "trashed" && !purge {
        return Err(WikiError::content_not_found(&id).into());
    }
    service::remove(&state, &user, &found, purge).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn children(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &user, &id, true).await?;
    let mut kids = content::children(&state.db, page.id, "page").await?;
    super::access::retain_visible(&state.db, &user, &mut kids).await?;
    let items: Vec<Value> = kids
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &user, &id, true).await?;
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
        // A page hidden by its restrictions hides its whole subtree.
        if !super::access::can_view(&state.db, &user, &node).await? {
            continue;
        }
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> V2Result<Json<Value>> {
    let page = find_item(&state, &user, &id, true).await?;
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &user, &id, true).await?;
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
    CurrentUser(user): CurrentUser,
    Path((id, number)): Path<(String, i64)>,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &user, &id, true).await?;
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &user, &id, true).await?;
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
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &user, &id, true).await?;
    let mut top: Vec<Content> = content::comments(&state.db, found.id)
        .await?
        .into_iter()
        .filter(|c| c.parent_id.is_none())
        .collect();
    retain_location(&state, &mut top, false).await?;
    let path = format!("/{}s/{id}/footer-comments", found.content_type);
    Ok(Json(comments_page(&state, &top, &params, &path).await?))
}

async fn list_footer_comments(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    let mut all = content::list(
        &state.db,
        &ListFilter {
            content_type: Some("comment".into()),
            ..Default::default()
        },
    )
    .await?;
    super::access::retain_visible(&state.db, &user, &mut all).await?;
    retain_location(&state, &mut all, false).await?;
    Ok(Json(
        comments_page(&state, &all, &params, "/footer-comments").await?,
    ))
}

// `/comments`: the first form of the v2 comments API, which some clients still call. It
// covers footer and inline comments alike.

async fn create_any_comment(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(request): Json<Value>,
) -> V2Result<Json<Value>> {
    let parse = |e: serde_json::Error| WikiError::bad_request(e.to_string());
    let inline_parent = match request.get("parentCommentId").and_then(service::content_id) {
        Some(id) => match find_comment(&state, &user.0, &id).await {
            Ok(parent) => inline_comments::get(&state.db, parent.id).await?.is_some(),
            Err(_) => false,
        },
        None => false,
    };
    if request.get("inlineCommentProperties").is_some() || inline_parent {
        let request: InlineRequest = serde_json::from_value(request).map_err(parse)?;
        create_inline_comment(State(state), user, Json(request)).await
    } else {
        let request: CommentRequest = serde_json::from_value(request).map_err(parse)?;
        create_footer_comment(State(state), user, Json(request)).await
    }
}

async fn get_any_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &user, &id).await?;
    Ok(Json(
        V2Ctx::new(&state)
            .comment(&found, body_format(&params)?)
            .await?,
    ))
}

async fn update_any_comment(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<Value>,
) -> V2Result<Json<Value>> {
    let parse = |e: serde_json::Error| WikiError::bad_request(e.to_string());
    let found = find_comment(&state, &user.0, &id).await?;
    if inline_comments::get(&state.db, found.id).await?.is_some() {
        let request: InlineUpdate = serde_json::from_value(request).map_err(parse)?;
        update_inline_comment(State(state), user, Path(id), Json(request)).await
    } else {
        let request: CommentRequest = serde_json::from_value(request).map_err(parse)?;
        update_footer_comment(State(state), user, Path(id), Json(request)).await
    }
}

// Inline comments.

/// Top-level inline comments on a page or blog post, optionally by `resolution-status`.
async fn inline_comments_of(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_item(&state, &user, &id, true).await?;
    let mut top: Vec<Content> = content::comments(&state.db, found.id)
        .await?
        .into_iter()
        .filter(|c| c.parent_id.is_none())
        .collect();
    retain_location(&state, &mut top, true).await?;
    let wanted = params.all("resolution-status");
    if !wanted.is_empty() {
        let mut kept = Vec::new();
        for item in top {
            if let Some(inline) = inline_comments::get(&state.db, item.id).await?
                && wanted.iter().any(|w| w == inline.status(&found.body))
            {
                kept.push(item);
            }
        }
        top = kept;
    }
    let path = format!("/{}s/{id}/inline-comments", found.content_type);
    Ok(Json(comments_page(&state, &top, &params, &path).await?))
}

async fn list_inline_comments(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> V2Result<Json<Value>> {
    let mut all = content::list(
        &state.db,
        &ListFilter {
            content_type: Some("comment".into()),
            ..Default::default()
        },
    )
    .await?;
    super::access::retain_visible(&state.db, &user, &mut all).await?;
    retain_location(&state, &mut all, true).await?;
    Ok(Json(
        comments_page(&state, &all, &params, "/inline-comments").await?,
    ))
}

async fn find_inline(state: &AppState, user: &User, id: &str) -> V2Result<Content> {
    let found = find_comment(state, user, id).await?;
    if inline_comments::get(&state.db, found.id).await?.is_none() {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_inline_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_inline(&state, &user, &id).await?;
    Ok(Json(
        V2Ctx::new(&state)
            .comment(&found, body_format(&params)?)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineRequest {
    page_id: Option<Value>,
    blog_post_id: Option<Value>,
    parent_comment_id: Option<Value>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    inline_comment_properties: Value,
}

async fn create_inline_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<InlineRequest>,
) -> V2Result<Json<Value>> {
    let body = request_body(&request.body)?.unwrap_or_default();
    let format = request_format(&request.body);
    // A reply goes on the same text as the comment it answers.
    if let Some(parent) = request
        .parent_comment_id
        .as_ref()
        .and_then(service::content_id)
    {
        let parent = find_inline(&state, &user, &parent).await?;
        let container_id = parent
            .container_id
            .ok_or_else(|| WikiError::bad_request("The comment is not on a page"))?;
        let container = find_item(&state, &user, &container_id.to_string(), true).await?;
        let created =
            service::create_comment(&state, &user, &container, Some(&parent), &body).await?;
        return Ok(Json(V2Ctx::new(&state).comment(&created, format).await?));
    }
    let container_id = request
        .page_id
        .as_ref()
        .or(request.blog_post_id.as_ref())
        .and_then(service::content_id)
        .ok_or_else(|| {
            WikiError::bad_request("pageId, blogPostId, or parentCommentId is required")
        })?;
    let container = find_item(&state, &user, &container_id, true).await?;
    let properties = &request.inline_comment_properties;
    let selection = properties["textSelection"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            WikiError::bad_request("inlineCommentProperties.textSelection is required")
        })?;
    let index = properties["textSelectionMatchIndex"].as_u64().unwrap_or(0) as usize;
    let created =
        service::create_inline_comment(&state, &user, &container, selection, index, &body).await?;
    Ok(Json(V2Ctx::new(&state).comment(&created, format).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineUpdate {
    #[serde(default)]
    body: Value,
    #[serde(default)]
    version: Value,
    resolved: Option<bool>,
}

/// `PUT /inline-comments/{id}`: a new body, or `resolved` to resolve or reopen the comment.
async fn update_inline_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<InlineUpdate>,
) -> V2Result<Json<Value>> {
    let found = find_inline(&state, &user, &id).await?;
    let inline = inline_comments::get(&state.db, found.id)
        .await?
        .expect("an inline comment");
    if let Some(resolved) = request.resolved {
        // Anyone who may comment on the page may resolve or reopen its comments.
        super::access::require(&state.db, &user, found.space_id, "create", "comment").await?;
        inline_comments::set_resolved(&state.db, &inline.marker_ref, resolved, user.id).await?;
    }
    let mut current = found.clone();
    if let Some(body) = request_body(&request.body)? {
        if found.creator_id != user.id && !user.is_admin {
            return Err(WikiError::new(
                StatusCode::FORBIDDEN,
                "Only the author can edit a comment",
            )
            .into());
        }
        let save = Save {
            number: request.version["number"]
                .as_i64()
                .ok_or_else(|| WikiError::bad_request("version.number is required"))?,
            title: None,
            body: Some(body),
            status: "current",
            parent: Parent::Keep,
            message: request.version["message"].as_str().unwrap_or_default(),
            minor_edit: false,
        };
        current = service::save(&state, &user, &found, save).await?;
    }
    let format = request_format(&request.body);
    Ok(Json(V2Ctx::new(&state).comment(&current, format).await?))
}

async fn find_comment(state: &AppState, user: &User, id: &str) -> V2Result<Content> {
    let found = find_current(state, user, id).await?;
    if found.content_type != "comment" {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_footer_comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &user, &id).await?;
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
        Some(id) => Some(find_comment(&state, &user, &id).await?),
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
    let container = find_item(&state, &user, &container_id, true).await?;
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
    let found = find_comment(&state, &user, &id).await?;
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
    let found = find_comment(&state, &user, &id).await?;
    if found.creator_id != user.id && !user.is_admin {
        return Err(WikiError::new(
            StatusCode::FORBIDDEN,
            "Only the author can delete a comment",
        )
        .into());
    }
    service::remove(&state, &user, &found, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn comment_replies(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let found = find_comment(&state, &user, &id).await?;
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

// Attachments.

async fn find_attachment(state: &AppState, user: &User, id: &str) -> V2Result<Content> {
    let found = find_content(state, user, id).await?;
    if found.content_type != "attachment" {
        return Err(WikiError::content_not_found(id).into());
    }
    Ok(found)
}

async fn get_attachment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> V2Result<Json<Value>> {
    let found = find_attachment(&state, &user, &id).await?;
    if found.status != "current" {
        return Err(WikiError::content_not_found(&id).into());
    }
    Ok(Json(V2Ctx::new(&state).attachment(&found).await?))
}

/// Moves an attachment to the trash; with `purge=true`, deletes a trashed one for good.
async fn delete_attachment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<StatusCode> {
    let found = find_attachment(&state, &user, &id).await?;
    let purge = params.get("purge") == Some("true");
    if found.status == "trashed" && !purge {
        return Err(WikiError::content_not_found(&id).into());
    }
    service::remove(&state, &user, &found, purge).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Attachments of a page or blog post, optionally filtered by `filename` and `mediaType`.
async fn attachments_of(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let container = find_item(&state, &user, &id, true).await?;
    let mut ctx = V2Ctx::new(&state);
    let mut items = Vec::new();
    for attachment in content::attachments(&state.db, container.id).await? {
        let rendered = ctx.attachment(&attachment).await?;
        let keep = params.get("filename").is_none_or(|f| f == attachment.title)
            && params
                .get("mediaType")
                .is_none_or(|m| rendered["mediaType"] == m);
        if keep {
            items.push(rendered);
        }
    }
    let path = format!("/{}s/{id}/attachments", container.content_type);
    Ok(Json(paged(&state, &path, &params, items)?))
}

/// `GET /spaces/{id}/permissions`: who may do what in a space.
async fn space_permissions_list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> V2Result<Json<Value>> {
    let space = find_space_by_id(&state, &user, &id).await?;
    let mut items = Vec::new();
    for permission in crate::store::space_permissions::for_space(&state.db, space.id).await? {
        let principal = match permission.subject_type.as_str() {
            "anonymous" => json!({ "type": "anonymous", "id": null }),
            kind => json!({ "type": kind, "id": permission.subject }),
        };
        items.push(json!({
            "id": permission.id.to_string(),
            "principal": principal,
            "operation": { "key": permission.operation, "targetType": permission.target },
        }));
    }
    Ok(Json(paged(
        &state,
        &format!("/spaces/{id}/permissions"),
        &params,
        items,
    )?))
}
