//! Content: pages, blog posts, and comments under `/content`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::access;
use super::error::{WikiError, WikiResult};
use super::render::{self, Expand, WikiCtx};
use super::service::{self, NewPage, Parent, Save, check_not_below, find_content, find_current};
use super::spaces::find_space;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content::{self, Content, ListFilter};
use crate::store::labels;
use crate::store::users::User;
use crate::time;

pub use service::content_id;

/// What create and update responses include, as Confluence's do.
const WRITE_EXPAND: &str = "space,history,version,ancestors,container,body.storage";

#[derive(Deserialize)]
pub struct ContentRequest {
    #[serde(rename = "type")]
    content_type: Option<String>,
    title: Option<String>,
    status: Option<String>,
    #[serde(default)]
    space: Value,
    #[serde(default)]
    ancestors: Vec<Value>,
    #[serde(default)]
    container: Value,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    version: Value,
}

/// The storage-format body of a v1 request, `{"storage": {"value": ...}}`, if it has one.
fn request_body(body: &Value) -> WikiResult<Option<String>> {
    match body
        .as_object()
        .and_then(|o| o.iter().find(|(k, _)| !k.starts_with('_')))
    {
        Some((representation, value)) => {
            service::body_from(representation, &value["value"]).map(Some)
        }
        None => Ok(None),
    }
}

/// The id of the last entry of `ancestors`: a page's parent, or the comment a reply answers.
fn last_ancestor(ancestors: &[Value]) -> WikiResult<Option<String>> {
    match ancestors.last() {
        Some(last) => content_id(&last["id"])
            .map(Some)
            .ok_or_else(|| WikiError::bad_request("Ancestors must have an id")),
        None => Ok(None),
    }
}

/// The response's expansions: the client's, or Confluence's defaults for writes.
fn write_expand(params: &Params) -> Expand {
    match params.get("expand") {
        Some(_) => params.expand(),
        None => Expand::parse(Some(WRITE_EXPAND)),
    }
}

pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
    Json(request): Json<ContentRequest>,
) -> WikiResult<Json<Value>> {
    let content_type = request.content_type.as_deref().unwrap_or("page");
    let body = request_body(&request.body)?.unwrap_or_default();
    let created = match content_type {
        "page" | "blogpost" => {
            let key = request.space["key"]
                .as_str()
                .ok_or_else(|| WikiError::bad_request("Space key is required"))?;
            let space = find_space(&state, &user, key).await?;
            let parent = match last_ancestor(&request.ancestors)? {
                Some(id) if content_type == "page" => {
                    Some(service::parent_page(&state, &user, &id, space.id).await?)
                }
                _ => None,
            };
            let page = NewPage {
                kind: content_type,
                status: request.status.as_deref().unwrap_or("current"),
                space: &space,
                title: request.title.as_deref().unwrap_or_default(),
                parent: parent.as_ref(),
                body: &body,
            };
            service::create_page(&state, &user, page).await?
        }
        "comment" => {
            let container_id = content_id(&request.container["id"])
                .ok_or_else(|| WikiError::bad_request("A comment needs a container"))?;
            let container = find_current(&state, &user, &container_id).await?;
            let reply_to = match last_ancestor(&request.ancestors)? {
                Some(id) => Some(find_current(&state, &user, &id).await?),
                None => None,
            };
            service::create_comment(&state, &user, &container, reply_to.as_ref(), &body).await?
        }
        other => {
            return Err(WikiError::bad_request(format!(
                "Unsupported content type: {other}"
            )));
        }
    };
    Ok(Json(
        WikiCtx::new(&state)
            .content(&created, &write_expand(&params))
            .await?,
    ))
}

/// `GET /content`: pages or blog posts, filtered by space, title, status, and posting day.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let (start, limit) = params.page(25, 500);
    let content_type = params.get("type").unwrap_or("page").to_string();
    let mut filter = ListFilter {
        content_type: Some(content_type),
        status: params.get("status").map(str::to_string),
        title: params.get("title").map(str::to_string),
        ..Default::default()
    };
    if let Some(key) = params.get("spaceKey") {
        match crate::store::spaces::by_key(&state.db, key).await? {
            Some(space) => filter.space_id = Some(space.id),
            None => {
                return Ok(Json(WikiCtx::new(&state).collection(
                    Vec::new(),
                    start,
                    limit,
                    "/content",
                    false,
                )));
            }
        }
    }
    if let Some(day) = params.get("postingDay") {
        let date = time::parse_date(day)
            .ok_or_else(|| WikiError::bad_request("postingDay must be yyyy-mm-dd"))?;
        let from = time::parse_jql_datetime(&date.to_string()).unwrap_or_default();
        filter.created_from = Some(from);
        filter.created_before = Some(from + 86_400_000);
    }
    let mut all = content::list(&state.db, &filter).await?;
    access::retain_readable(&state.db, &user, &mut all).await?;
    let more = all.len() as i64 > start + limit;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(&state);
    let mut results = Vec::new();
    for item in all.iter().skip(start as usize).take(limit as usize) {
        results.push(ctx.content(item, &expand).await?);
    }
    Ok(Json(
        ctx.collection(results, start, limit, "/content", more),
    ))
}

/// `GET /content/{id}`. `version` reads an older version; `status` allows trashed content.
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let mut found = find_content(&state, &user, &id).await?;
    let statuses = params.all("status");
    let allowed = if statuses.is_empty() {
        found.status == "current"
    } else {
        statuses.iter().any(|s| *s == found.status || s == "any")
    };
    if !allowed {
        return Err(WikiError::content_not_found(&id));
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
    // Confluence expands these by default when reading one piece of content.
    let expand = match params.get("expand") {
        Some(_) => params.expand(),
        None => Expand::parse(Some("space,history,version")),
    };
    Ok(Json(WikiCtx::new(&state).content(&found, &expand).await?))
}

/// `PUT /content/{id}`: saves a new version. The version number must be one more than the
/// current one, so two people cannot overwrite each other.
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
    Json(request): Json<ContentRequest>,
) -> WikiResult<Json<Value>> {
    let found = find_content(&state, &user, &id).await?;
    let number = request.version["number"]
        .as_i64()
        .ok_or_else(|| WikiError::bad_request("Version number is required to update content"))?;
    if let Some(kind) = &request.content_type
        && *kind != found.content_type
    {
        return Err(WikiError::bad_request(format!(
            "Changing the type of content from {} to {kind} is not supported",
            found.content_type
        )));
    }
    let parent = match last_ancestor(&request.ancestors)? {
        Some(id) if found.content_type == "page" => Parent::Move(Some(
            service::parent_page(&state, &user, &id, found.space_id).await?,
        )),
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
    Ok(Json(
        WikiCtx::new(&state)
            .content(&updated, &write_expand(&params))
            .await?,
    ))
}

/// `DELETE /content/{id}`: moves current content to the trash. With `status=trashed`, purges
/// content that is already in the trash. Drafts are deleted at once.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let found = find_content(&state, &user, &id).await?;
    // Trashed content is invisible unless the client names its status.
    if found.status == "trashed" && params.get("status") != Some("trashed") {
        return Err(WikiError::content_not_found(&id));
    }
    service::remove(&state, &user, &found, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn children(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    Ok(Json(
        WikiCtx::new(&state)
            .children(&found, &params.expand())
            .await?,
    ))
}

async fn list_of(
    state: &AppState,
    items: Vec<Content>,
    params: &Params,
    path: String,
) -> WikiResult<Value> {
    let (start, limit) = params.page(25, 500);
    let more = items.len() as i64 > start + limit;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(state);
    let mut results = Vec::new();
    for item in items.iter().skip(start as usize).take(limit as usize) {
        results.push(ctx.content(item, &expand).await?);
    }
    Ok(ctx.collection(results, start, limit, &path, more))
}

pub async fn children_of_type(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    // `depth=all` lists replies along with top-level comments.
    let items = if kind == "comment" && params.get("depth") == Some("all") {
        render::descendant_list(&state.db, &found, &kind).await?
    } else {
        render::child_list(&state.db, &found, &kind).await?
    };
    let path = format!("/content/{id}/child/{kind}");
    Ok(Json(list_of(&state, items, &params, path).await?))
}

pub async fn descendants(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    let expand = params.expand().with("page").with("comment");
    let mut ctx = WikiCtx::new(&state);
    let mut out = serde_json::Map::new();
    for kind in ["page", "comment"] {
        let items = render::descendant_list(&state.db, &found, kind).await?;
        let mut results = Vec::new();
        for item in &items {
            results.push(ctx.content(item, &expand.below(kind)).await?);
        }
        let count = results.len() as i64;
        let path = format!("/content/{id}/descendant/{kind}");
        out.insert(
            kind.into(),
            ctx.collection(results, 0, count.max(25), &path, false),
        );
    }
    Ok(Json(Value::Object(out)))
}

pub async fn descendants_of_type(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    let items = render::descendant_list(&state.db, &found, &kind).await?;
    let path = format!("/content/{id}/descendant/{kind}");
    Ok(Json(list_of(&state, items, &params, path).await?))
}

pub async fn history(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    // This endpoint includes the last update unless the client picks its own expansions.
    let expand = match params.get("expand") {
        Some(_) => params.expand(),
        None => Expand::parse(Some("lastUpdated")),
    };
    Ok(Json(WikiCtx::new(&state).history(&found, &expand).await?))
}

pub async fn versions(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    let (start, limit) = params.page(200, 200);
    let all = content::versions(&state.db, found.id).await?;
    let more = all.len() as i64 > start + limit;
    let mut ctx = WikiCtx::new(&state);
    let mut results = Vec::new();
    for version in all.iter().skip(start as usize).take(limit as usize) {
        results.push(ctx.version(version, &found).await?);
    }
    Ok(Json(ctx.collection(
        results,
        start,
        limit,
        &format!("/content/{id}/version"),
        more,
    )))
}

pub async fn version(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, number)): Path<(String, i64)>,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    let version = content::version(&state.db, found.id, number)
        .await?
        .ok_or_else(|| WikiError::not_found(format!("No version {number} of content {id}")))?;
    Ok(Json(WikiCtx::new(&state).version(&version, &found).await?))
}

/// `POST /content/{id}/version` with `operationKey: restore` saves an old version as the newest.
pub async fn restore_version(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<Value>,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    if request["operationKey"].as_str() != Some("restore") {
        return Err(WikiError::bad_request("operationKey must be restore"));
    }
    let number = request["params"]["versionNumber"]
        .as_i64()
        .ok_or_else(|| WikiError::bad_request("params.versionNumber is required"))?;
    let old = content::version(&state.db, found.id, number)
        .await?
        .ok_or_else(|| WikiError::not_found(format!("No version {number} of content {id}")))?;
    let restore_title = request["params"]["restoreTitle"].as_bool().unwrap_or(false);
    let save = Save {
        number: found.version + 1,
        title: restore_title.then_some(old.title.as_str()),
        body: Some(old.body.clone()),
        status: "current",
        parent: Parent::Keep,
        message: request["params"]["message"].as_str().unwrap_or_default(),
        minor_edit: false,
    };
    let updated = service::save(&state, &user, &found, save).await?;
    let latest = content::version(&state.db, updated.id, updated.version)
        .await?
        .expect("the new version exists");
    Ok(Json(WikiCtx::new(&state).version(&latest, &updated).await?))
}

pub async fn labels(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    let mut list = labels::for_content(&state.db, found.id).await?;
    if let Some(prefix) = params.get("prefix") {
        list.retain(|l| l.prefix == prefix);
    }
    let (start, limit) = params.page(200, 200);
    let more = list.len() as i64 > start + limit;
    let ctx = WikiCtx::new(&state);
    let results = list
        .iter()
        .skip(start as usize)
        .take(limit as usize)
        .map(|l| ctx.label(l))
        .collect();
    Ok(Json(ctx.collection(
        results,
        start,
        limit,
        &format!("/content/{id}/label"),
        more,
    )))
}

/// Confluence stores labels in lower case and does not allow spaces in them.
fn label_name(name: &str) -> WikiResult<String> {
    let name = name.trim().to_lowercase();
    if name.is_empty() || name.len() > 255 {
        return Err(WikiError::bad_request(
            "Label names must be 1 to 255 characters",
        ));
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || ":;,.?&[]()#^*@!".contains(c))
    {
        return Err(WikiError::bad_request(format!(
            "Label name '{name}' contains invalid characters"
        )));
    }
    Ok(name)
}

pub async fn add_labels(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<Value>,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &user, &id).await?;
    access::require(&state.db, &user, found.space_id, "create", "page").await?;
    // The body is one label or a list of labels.
    let items = match request {
        Value::Array(items) => items,
        single => vec![single],
    };
    for item in &items {
        let name = label_name(item["name"].as_str().unwrap_or_default())?;
        let prefix = item["prefix"].as_str().unwrap_or("global");
        if !["global", "my", "team"].contains(&prefix) {
            return Err(WikiError::bad_request(format!(
                "Invalid label prefix: {prefix}"
            )));
        }
        labels::add(&state.db, found.id, prefix, &name).await?;
    }
    Ok(Json(WikiCtx::new(&state).labels(found.id).await?))
}

async fn remove_label_named(
    state: &AppState,
    user: &User,
    id: &str,
    name: &str,
) -> WikiResult<StatusCode> {
    let found = find_current(state, user, id).await?;
    access::require(&state.db, user, found.space_id, "create", "page").await?;
    if !labels::remove(&state.db, found.id, &name.to_lowercase()).await? {
        return Err(WikiError::not_found(format!(
            "Label {name} is not on content {id}"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_label(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, label)): Path<(String, String)>,
) -> WikiResult<StatusCode> {
    remove_label_named(&state, &user, &id, &label).await
}

pub async fn remove_label_by_query(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let name = params
        .get("name")
        .ok_or_else(|| WikiError::bad_request("name is required"))?
        .to_string();
    remove_label_named(&state, &user, &id, &name).await
}

/// `PUT /content/{id}/move/{position}/{targetId}`: `before` or `after` a sibling, or `append`
/// as the last child of the target.
pub async fn move_page(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, position, target)): Path<(String, String, String)>,
) -> WikiResult<Json<Value>> {
    let page = find_current(&state, &user, &id).await?;
    let target = find_current(&state, &user, &target).await?;
    access::require(&state.db, &user, page.space_id, "create", "page").await?;
    if page.content_type != "page" || target.content_type != "page" {
        return Err(WikiError::bad_request("Only pages can be moved"));
    }
    if target.space_id != page.space_id {
        return Err(WikiError::bad_request(
            "Moving pages between spaces is not supported yet",
        ));
    }
    let new_parent = match position.as_str() {
        "append" => Some(target.clone()),
        "before" | "after" => match target.parent_id {
            Some(parent) => content::by_id(&state.db, parent).await?,
            None => None,
        },
        other => return Err(WikiError::bad_request(format!("Invalid position: {other}"))),
    };
    if let Some(parent) = &new_parent {
        check_not_below(&state, &page, parent).await?;
    }
    let parent_id = new_parent.as_ref().map(|p| p.id);
    if parent_id != page.parent_id {
        // Confluence moves pages without saving a new version.
        content::set_parent(&state.db, page.id, parent_id).await?;
    }
    // Rebuild the sibling order with the page in its new place.
    let mut siblings: Vec<i64> = match parent_id {
        Some(parent) => content::children(&state.db, parent, "page").await?,
        None => content::root_pages(&state.db, page.space_id).await?,
    }
    .into_iter()
    .map(|c| c.id)
    .filter(|c| *c != page.id)
    .collect();
    let index = match position.as_str() {
        "append" => siblings.len(),
        "before" => siblings.iter().position(|s| *s == target.id).unwrap_or(0),
        _ => siblings
            .iter()
            .position(|s| *s == target.id)
            .map_or(siblings.len(), |i| i + 1),
    };
    siblings.insert(index, page.id);
    content::reorder(&state.db, &page, &siblings).await?;
    Ok(Json(json!({ "pageId": page.id.to_string() })))
}
