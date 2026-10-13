//! Content: pages, blog posts, and comments under `/content`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::error::{WikiError, WikiResult};
use super::render::{self, Expand, WikiCtx};
use super::spaces::find_space;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content::{self, Content, Edit, ListFilter, NewContent};
use crate::store::labels;
use crate::time::{self, now_ms};

/// What create and update responses include, as Confluence's do.
const WRITE_EXPAND: &str = "space,history,version,ancestors,container,body.storage";

/// An id given as a JSON string or number.
pub fn content_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

/// Content by id, in any status.
pub async fn find_content(state: &AppState, id: &str) -> WikiResult<Content> {
    let found = match id.parse::<i64>() {
        Ok(number) => content::by_id(&state.db, number).await?,
        Err(_) => None,
    };
    found.ok_or_else(|| WikiError::content_not_found(id))
}

/// Content that is current (not trashed or a draft), as most reads require.
async fn find_current(state: &AppState, id: &str) -> WikiResult<Content> {
    let found = find_content(state, id).await?;
    if found.status != "current" {
        return Err(WikiError::content_not_found(id));
    }
    Ok(found)
}

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

/// The storage-format body of a request, if it has one.
fn request_body(body: &Value) -> WikiResult<Option<String>> {
    let Some(object) = body.as_object() else {
        return Ok(None);
    };
    for representation in ["storage", "editor"] {
        if let Some(value) = object.get(representation) {
            return Ok(Some(
                value["value"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    match object.keys().next() {
        Some(other) => Err(WikiError::bad_request(format!(
            "Unsupported body representation: {other}. Use storage."
        ))),
        None => Ok(None),
    }
}

/// The parent named by the last entry of `ancestors`, which must be a page in the space.
async fn parent_page(
    state: &AppState,
    ancestors: &[Value],
    space_id: i64,
) -> WikiResult<Option<Content>> {
    let Some(last) = ancestors.last() else {
        return Ok(None);
    };
    let id = content_id(&last["id"])
        .ok_or_else(|| WikiError::bad_request("Ancestors must have an id"))?;
    let parent = find_current(state, &id).await?;
    if parent.content_type != "page" || parent.space_id != space_id {
        return Err(WikiError::bad_request(format!(
            "The parent page {id} must be a page in the same space"
        )));
    }
    Ok(Some(parent))
}

async fn check_title(
    state: &AppState,
    space_id: i64,
    content_type: &str,
    title: &str,
    except: Option<i64>,
) -> WikiResult<()> {
    if title.trim().is_empty() {
        return Err(WikiError::bad_request("Content title cannot be empty"));
    }
    if title.chars().count() > 255 {
        return Err(WikiError::bad_request(
            "Title cannot be longer than 255 characters",
        ));
    }
    if content_type == "page"
        && content::title_taken(&state.db, space_id, content_type, title, except).await?
    {
        let key = crate::store::spaces::by_id(&state.db, space_id)
            .await?
            .map(|s| s.key)
            .unwrap_or_default();
        return Err(WikiError::bad_request(format!(
            "A page with this title already exists: A page already exists with the title {title} in the space with key {key}"
        )));
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
    Json(request): Json<ContentRequest>,
) -> WikiResult<Json<Value>> {
    let content_type = request.content_type.as_deref().unwrap_or("page");
    let status = request.status.as_deref().unwrap_or("current");
    if status != "current" && status != "draft" {
        return Err(WikiError::bad_request(format!(
            "Unsupported status: {status}"
        )));
    }
    let body = request_body(&request.body)?.unwrap_or_default();
    let now = now_ms();
    let created = match content_type {
        "page" | "blogpost" => {
            let key = request.space["key"]
                .as_str()
                .ok_or_else(|| WikiError::bad_request("Space key is required"))?;
            let space = find_space(&state, key).await?;
            let title = request.title.unwrap_or_default();
            if status == "current" {
                check_title(&state, space.id, content_type, &title, None).await?;
            }
            let parent = if content_type == "page" {
                parent_page(&state, &request.ancestors, space.id).await?
            } else {
                None
            };
            content::create(
                &state.db,
                &NewContent {
                    content_type,
                    status,
                    space_id: space.id,
                    parent_id: parent.map(|p| p.id),
                    container_id: None,
                    title: &title,
                    body: &body,
                    author_id: user.id,
                    now,
                },
            )
            .await?
        }
        "comment" => {
            let container_id = content_id(&request.container["id"])
                .ok_or_else(|| WikiError::bad_request("A comment needs a container"))?;
            let container = find_current(&state, &container_id).await?;
            if container.content_type == "comment" {
                return Err(WikiError::bad_request(
                    "A comment's container must be a page or blog post",
                ));
            }
            // A reply names the comment it answers as its last ancestor.
            let reply_to = match request.ancestors.last().and_then(|a| content_id(&a["id"])) {
                Some(id) => {
                    let parent = find_current(&state, &id).await?;
                    if parent.content_type != "comment" || parent.container_id != Some(container.id)
                    {
                        return Err(WikiError::bad_request(
                            "A reply must answer a comment on the same content",
                        ));
                    }
                    Some(parent.id)
                }
                None => None,
            };
            if body.trim().is_empty() {
                return Err(WikiError::bad_request("Comment body cannot be empty"));
            }
            content::create(
                &state.db,
                &NewContent {
                    content_type: "comment",
                    status: "current",
                    space_id: container.space_id,
                    parent_id: reply_to,
                    container_id: Some(container.id),
                    title: &format!("Re: {}", container.title),
                    body: &body,
                    author_id: user.id,
                    now,
                },
            )
            .await?
        }
        other => {
            return Err(WikiError::bad_request(format!(
                "Unsupported content type: {other}"
            )));
        }
    };
    let expand = params.expand();
    let expand = if params.get("expand").is_some() {
        expand
    } else {
        Expand::parse(Some(WRITE_EXPAND))
    };
    Ok(Json(WikiCtx::new(&state).content(&created, &expand).await?))
}

/// `GET /content`: pages or blog posts, filtered by space, title, status, and posting day.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
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
    let all = content::list(&state.db, &filter).await?;
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
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let mut found = find_content(&state, &id).await?;
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
    let found = find_content(&state, &id).await?;
    let number = request.version["number"]
        .as_i64()
        .ok_or_else(|| WikiError::bad_request("Version number is required to update content"))?;
    if number != found.version + 1 {
        return Err(WikiError::conflict(format!(
            "Version must be incremented on update. Current version is: {}",
            found.version
        )));
    }
    if let Some(kind) = &request.content_type
        && *kind != found.content_type
    {
        return Err(WikiError::bad_request(format!(
            "Changing the type of content from {} to {kind} is not supported",
            found.content_type
        )));
    }
    let status = request.status.clone().unwrap_or_else(|| "current".into());
    if status != "current" && status != "draft" {
        return Err(WikiError::bad_request(format!(
            "Unsupported status: {status}"
        )));
    }
    let title = request.title.clone().unwrap_or_else(|| found.title.clone());
    if found.content_type != "comment" {
        check_title(
            &state,
            found.space_id,
            &found.content_type,
            &title,
            Some(found.id),
        )
        .await?;
    }
    let body = request_body(&request.body)?.unwrap_or_else(|| found.body.clone());
    let parent_id = if found.content_type == "page" && !request.ancestors.is_empty() {
        let parent = parent_page(&state, &request.ancestors, found.space_id).await?;
        if let Some(parent) = &parent {
            check_not_below(&state, &found, parent).await?;
        }
        parent.map(|p| p.id)
    } else if found.status == "trashed" {
        // Restored pages return to the top of the space, as their old parent may be gone.
        None
    } else {
        found.parent_id
    };
    let updated = content::update(
        &state.db,
        &found,
        &Edit {
            title: &title,
            body: &body,
            status: &status,
            parent_id,
            author_id: user.id,
            now: now_ms(),
            message: request.version["message"].as_str().unwrap_or_default(),
            minor_edit: request.version["minorEdit"].as_bool().unwrap_or(false),
        },
    )
    .await?;
    let expand = match params.get("expand") {
        Some(_) => params.expand(),
        None => Expand::parse(Some(WRITE_EXPAND)),
    };
    Ok(Json(WikiCtx::new(&state).content(&updated, &expand).await?))
}

/// Refuses to move a page below itself.
async fn check_not_below(state: &AppState, page: &Content, new_parent: &Content) -> WikiResult<()> {
    if new_parent.id == page.id {
        return Err(WikiError::bad_request("A page cannot be its own parent"));
    }
    for ancestor in content::ancestors(&state.db, new_parent).await? {
        if ancestor.id == page.id {
            return Err(WikiError::bad_request(
                "A page cannot move below one of its own children",
            ));
        }
    }
    Ok(())
}

/// `DELETE /content/{id}`: moves current content to the trash. With `status=trashed`, purges
/// content that is already in the trash. Drafts are deleted at once.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let found = find_content(&state, &id).await?;
    match (found.status.as_str(), params.get("status")) {
        ("current", _) if found.content_type == "comment" => {
            content::purge(&state.db, found.id).await?
        }
        ("current", _) => content::trash(&state.db, &found, now_ms()).await?,
        ("trashed", Some("trashed")) | ("draft", _) => content::purge(&state.db, found.id).await?,
        _ => return Err(WikiError::content_not_found(&id)),
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn children(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    CurrentUser(_): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    CurrentUser(_): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
    let items = render::descendant_list(&state.db, &found, &kind).await?;
    let path = format!("/content/{id}/descendant/{kind}");
    Ok(Json(list_of(&state, items, &params, path).await?))
}

pub async fn history(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
    // This endpoint includes the last update unless the client picks its own expansions.
    let expand = match params.get("expand") {
        Some(_) => params.expand(),
        None => Expand::parse(Some("lastUpdated")),
    };
    Ok(Json(WikiCtx::new(&state).history(&found, &expand).await?))
}

pub async fn versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    CurrentUser(_): CurrentUser,
    Path((id, number)): Path<(String, i64)>,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    let found = find_current(&state, &id).await?;
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
    let title = if restore_title {
        old.title.clone()
    } else {
        found.title.clone()
    };
    if found.content_type != "comment" {
        check_title(
            &state,
            found.space_id,
            &found.content_type,
            &title,
            Some(found.id),
        )
        .await?;
    }
    let updated = content::update(
        &state.db,
        &found,
        &Edit {
            title: &title,
            body: &old.body,
            status: "current",
            parent_id: found.parent_id,
            author_id: user.id,
            now: now_ms(),
            message: request["params"]["message"].as_str().unwrap_or_default(),
            minor_edit: false,
        },
    )
    .await?;
    let latest = content::version(&state.db, updated.id, updated.version)
        .await?
        .expect("the new version exists");
    Ok(Json(WikiCtx::new(&state).version(&latest, &updated).await?))
}

pub async fn labels(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<Value>,
) -> WikiResult<Json<Value>> {
    let found = find_current(&state, &id).await?;
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

async fn remove_label_named(state: &AppState, id: &str, name: &str) -> WikiResult<StatusCode> {
    let found = find_current(state, id).await?;
    if !labels::remove(&state.db, found.id, &name.to_lowercase()).await? {
        return Err(WikiError::not_found(format!(
            "Label {name} is not on content {id}"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_label(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, label)): Path<(String, String)>,
) -> WikiResult<StatusCode> {
    remove_label_named(&state, &id, &label).await
}

pub async fn remove_label_by_query(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let name = params
        .get("name")
        .ok_or_else(|| WikiError::bad_request("name is required"))?
        .to_string();
    remove_label_named(&state, &id, &name).await
}

/// `PUT /content/{id}/move/{position}/{targetId}`: `before` or `after` a sibling, or `append`
/// as the last child of the target.
pub async fn move_page(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, position, target)): Path<(String, String, String)>,
) -> WikiResult<Json<Value>> {
    let page = find_current(&state, &id).await?;
    let target = find_current(&state, &target).await?;
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
