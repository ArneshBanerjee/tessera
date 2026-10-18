//! Page restrictions: `/content/{id}/restriction` and its `byOperation` forms.
//!
//! A page or blog post can be limited, for viewing (`read`) and for editing (`update`), to
//! named users and groups. Reading the restrictions needs only access to the page. Changing
//! them needs the space's "Restrict" permission and edit access to the page.
//!
//! As in Confluence, people cannot lock themselves out: when a change leaves an operation
//! restricted to others only, the caller is added to it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use super::Params;
use super::access;
use super::error::{WikiError, WikiResult};
use super::render::WikiCtx;
use super::service::find_current;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content::Content;
use crate::store::groups;
use crate::store::restrictions::{self, Restriction};
use crate::store::users::{self, User};

const OPERATIONS: [&str; 2] = ["read", "update"];

fn check_operation(operation: &str) -> WikiResult<()> {
    if OPERATIONS.contains(&operation) {
        Ok(())
    } else {
        Err(WikiError::bad_request(format!(
            "Unsupported restriction operation: {operation}"
        )))
    }
}

/// A page or blog post the user can see.
async fn restrictable(state: &AppState, user: &User, id: &str) -> WikiResult<Content> {
    let found = find_current(state, user, id).await?;
    if found.content_type != "page" && found.content_type != "blogpost" {
        return Err(WikiError::bad_request(
            "Restrictions apply to pages and blog posts only",
        ));
    }
    Ok(found)
}

/// A page whose restrictions the user may change.
async fn changeable(state: &AppState, user: &User, id: &str) -> WikiResult<Content> {
    let found = restrictable(state, user, id).await?;
    access::require(&state.db, user, found.space_id, "restrict_content", "space").await?;
    if !access::can_edit(&state.db, user, &found).await? {
        return Err(access::denied("restrict", &found.content_type));
    }
    Ok(found)
}

fn group_json(ctx: &WikiCtx<'_>, name: &str, group_id: Option<&str>) -> Value {
    json!({
        "type": "group",
        "name": name,
        "id": group_id,
        "_links": { "self": ctx.api_url(&format!("/group/by-name?name={name}")) },
    })
}

fn subjects(results: Vec<Value>) -> Value {
    let size = results.len();
    json!({ "results": results, "start": 0, "limit": 200, "size": size })
}

/// One operation's restrictions, in Confluence's shape.
async fn operation_json(
    ctx: &mut WikiCtx<'_>,
    content: &Content,
    operation: &str,
    all: &[Restriction],
) -> WikiResult<Value> {
    let mut people = Vec::new();
    let mut teams = Vec::new();
    for r in all.iter().filter(|r| r.operation == operation) {
        match r.subject_type.as_str() {
            "user" => {
                if let Some(found) = users::by_account_id(&ctx.state.db, &r.subject).await? {
                    people.push(ctx.user(&found));
                }
            }
            _ => {
                let group = groups::find(&ctx.state.db, &r.subject).await?;
                teams.push(group_json(
                    ctx,
                    &r.subject,
                    group.as_ref().map(|g| g.group_id.as_str()),
                ));
            }
        }
    }
    let id = content.id;
    Ok(json!({
        "operation": operation,
        "restrictions": { "user": subjects(people), "group": subjects(teams) },
        "_expandable": { "content": format!("/rest/api/content/{id}") },
        "_links": {
            "self": ctx.api_url(&format!("/content/{id}/restriction/byOperation/{operation}")),
        },
    }))
}

/// Both operations, keyed by name, as `byOperation` and the `restrictions` expansion show them.
pub async fn by_operation_json(ctx: &mut WikiCtx<'_>, content: &Content) -> WikiResult<Value> {
    let all = restrictions::of_content(&ctx.state.db, content.id).await?;
    let mut out = Map::new();
    for operation in OPERATIONS {
        out.insert(
            operation.into(),
            operation_json(ctx, content, operation, &all).await?,
        );
    }
    Ok(Value::Object(out))
}

/// Both operations as a list, as `/restriction` shows them.
async fn list_json(ctx: &mut WikiCtx<'_>, content: &Content) -> WikiResult<Value> {
    let all = restrictions::of_content(&ctx.state.db, content.id).await?;
    let mut results = Vec::new();
    for operation in OPERATIONS {
        results.push(operation_json(ctx, content, operation, &all).await?);
    }
    let path = format!("/content/{}/restriction", content.id);
    let mut out = ctx.collection(results, 0, 100, &path, false);
    // Lets clients tell whether restrictions changed since they last looked.
    out["restrictionsHash"] = json!(format!("{:x}", fingerprint(&all)));
    Ok(out)
}

/// A cheap fingerprint of a restriction list.
fn fingerprint(all: &[Restriction]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for r in all {
        (&r.operation, &r.subject_type, &r.subject).hash(&mut hasher);
    }
    hasher.finish()
}

/// Subjects named in a request body, as `(subject_type, subject)`.
async fn requested_subjects(state: &AppState, list: &Value) -> WikiResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    for kind in ["user", "group"] {
        // Each list may come bare or wrapped in `results`.
        let entries = match &list[kind] {
            Value::Array(items) => items.clone(),
            Value::Object(map) => map
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for entry in entries {
            out.push(match kind {
                "user" => {
                    let id = entry["accountId"].as_str().unwrap_or_default();
                    let found = users::by_account_id(&state.db, id).await?.ok_or_else(|| {
                        WikiError::bad_request(format!("No user with account id {id}"))
                    })?;
                    ("user".into(), found.account_id)
                }
                _ => {
                    let name = entry["name"]
                        .as_str()
                        .or_else(|| entry["id"].as_str())
                        .unwrap_or_default();
                    let found = groups::find(&state.db, name)
                        .await?
                        .ok_or_else(|| WikiError::bad_request(format!("No group named {name}")))?;
                    ("group".into(), found.name)
                }
            });
        }
    }
    Ok(out)
}

/// Applies a request body: a list of `{operation, restrictions: {user, group}}`, bare or
/// wrapped in `results`. With `replace`, each named operation loses its old subjects first.
async fn apply(state: &AppState, content: &Content, body: &Value, replace: bool) -> WikiResult<()> {
    let items = body
        .as_array()
        .or_else(|| body["results"].as_array())
        .ok_or_else(|| WikiError::bad_request("Expected a list of restrictions"))?;
    // Check the whole request before changing anything.
    let mut changes = Vec::new();
    for item in items {
        let operation = item["operation"].as_str().unwrap_or_default().to_string();
        check_operation(&operation)?;
        changes.push((
            operation,
            requested_subjects(state, &item["restrictions"]).await?,
        ));
    }
    for (operation, subjects) in changes {
        if replace {
            restrictions::clear(&state.db, content.id, Some(&operation)).await?;
        }
        for (kind, subject) in subjects {
            restrictions::add(&state.db, content.id, &operation, &kind, &subject).await?;
        }
    }
    Ok(())
}

/// Adds the caller to each restricted operation that no longer includes them.
async fn keep_caller(state: &AppState, user: &User, content: &Content) -> WikiResult<()> {
    if user.is_admin {
        return Ok(());
    }
    let mine: Vec<String> = groups::of_user(&state.db, user.id)
        .await?
        .into_iter()
        .map(|g| g.name.to_lowercase())
        .collect();
    let all = restrictions::of_content(&state.db, content.id).await?;
    for operation in OPERATIONS {
        let named: Vec<&Restriction> = all.iter().filter(|r| r.operation == operation).collect();
        let included = named.iter().any(|r| match r.subject_type.as_str() {
            "user" => r.subject == user.account_id,
            _ => mine.contains(&r.subject.to_lowercase()),
        });
        if !named.is_empty() && !included {
            restrictions::add(&state.db, content.id, operation, "user", &user.account_id).await?;
        }
    }
    Ok(())
}

/// `GET /content/{id}/restriction`.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> WikiResult<Json<Value>> {
    let found = restrictable(&state, &user, &id).await?;
    let mut ctx = WikiCtx::new(&state, &user);
    Ok(Json(list_json(&mut ctx, &found).await?))
}

async fn write(
    state: &AppState,
    user: &User,
    id: &str,
    body: Option<&Value>,
    replace: bool,
) -> WikiResult<Json<Value>> {
    let found = changeable(state, user, id).await?;
    match body {
        Some(body) => apply(state, &found, body, replace).await?,
        None => restrictions::clear(&state.db, found.id, None).await?,
    }
    keep_caller(state, user, &found).await?;
    let mut ctx = WikiCtx::new(state, user);
    Ok(Json(list_json(&mut ctx, &found).await?))
}

/// `PUT /content/{id}/restriction`: replaces the restrictions of the operations named.
pub async fn replace(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> WikiResult<Json<Value>> {
    write(&state, &user, &id, Some(&body), true).await
}

/// `POST /content/{id}/restriction`: adds to the restrictions.
pub async fn add(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> WikiResult<Json<Value>> {
    write(&state, &user, &id, Some(&body), false).await
}

/// `DELETE /content/{id}/restriction`: removes every restriction.
pub async fn clear(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> WikiResult<Json<Value>> {
    write(&state, &user, &id, None, false).await
}

/// `GET /content/{id}/restriction/byOperation`.
pub async fn by_operation(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> WikiResult<Json<Value>> {
    let found = restrictable(&state, &user, &id).await?;
    let mut ctx = WikiCtx::new(&state, &user);
    let mut out = by_operation_json(&mut ctx, &found).await?;
    out["_links"] =
        json!({ "self": ctx.api_url(&format!("/content/{id}/restriction/byOperation")) });
    Ok(Json(out))
}

/// `GET /content/{id}/restriction/byOperation/{operation}`.
pub async fn one_operation(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation)): Path<(String, String)>,
) -> WikiResult<Json<Value>> {
    check_operation(&operation)?;
    let found = restrictable(&state, &user, &id).await?;
    let all = restrictions::of_content(&state.db, found.id).await?;
    let mut ctx = WikiCtx::new(&state, &user);
    Ok(Json(
        operation_json(&mut ctx, &found, &operation, &all).await?,
    ))
}

/// What one subject request is about.
enum Subject {
    User(Params),
    /// A group by name or by group id.
    Group(String),
}

async fn resolve(state: &AppState, subject: Subject) -> WikiResult<(&'static str, String)> {
    match subject {
        Subject::User(params) => {
            let id = params
                .get("accountId")
                .ok_or_else(|| WikiError::bad_request("accountId is required"))?;
            let found = users::by_account_id(&state.db, id)
                .await?
                .ok_or_else(|| WikiError::bad_request(format!("No user with account id {id}")))?;
            Ok(("user", found.account_id))
        }
        Subject::Group(name) => {
            let found = groups::find(&state.db, &name)
                .await?
                .ok_or_else(|| WikiError::bad_request(format!("No group named {name}")))?;
            Ok(("group", found.name))
        }
    }
}

/// Whether a subject is named in an operation's restrictions: 200 if so, 404 if not.
async fn subject_status(
    state: &AppState,
    user: &User,
    id: &str,
    operation: &str,
    subject: Subject,
) -> WikiResult<StatusCode> {
    check_operation(operation)?;
    let found = restrictable(state, user, id).await?;
    let (kind, name) = resolve(state, subject).await?;
    let named = restrictions::of_content(&state.db, found.id)
        .await?
        .iter()
        .any(|r| r.operation == operation && r.subject_type == kind && r.subject == name);
    if named {
        Ok(StatusCode::OK)
    } else {
        Err(WikiError::not_found(
            "The subject is not in this restriction",
        ))
    }
}

async fn subject_add(
    state: &AppState,
    user: &User,
    id: &str,
    operation: &str,
    subject: Subject,
) -> WikiResult<StatusCode> {
    check_operation(operation)?;
    let found = changeable(state, user, id).await?;
    let (kind, name) = resolve(state, subject).await?;
    restrictions::add(&state.db, found.id, operation, kind, &name).await?;
    keep_caller(state, user, &found).await?;
    Ok(StatusCode::OK)
}

async fn subject_remove(
    state: &AppState,
    user: &User,
    id: &str,
    operation: &str,
    subject: Subject,
) -> WikiResult<StatusCode> {
    check_operation(operation)?;
    let found = changeable(state, user, id).await?;
    let (kind, name) = resolve(state, subject).await?;
    if !restrictions::remove(&state.db, found.id, operation, kind, &name).await? {
        return Err(WikiError::not_found(
            "The subject is not in this restriction",
        ));
    }
    keep_caller(state, user, &found).await?;
    Ok(StatusCode::OK)
}

/// `GET /content/{id}/restriction/byOperation/{operation}/user?accountId=`.
pub async fn user_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation)): Path<(String, String)>,
    params: Params,
) -> WikiResult<StatusCode> {
    subject_status(&state, &user, &id, &operation, Subject::User(params)).await
}

/// `PUT /content/{id}/restriction/byOperation/{operation}/user?accountId=`.
pub async fn user_add(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation)): Path<(String, String)>,
    params: Params,
) -> WikiResult<StatusCode> {
    subject_add(&state, &user, &id, &operation, Subject::User(params)).await
}

/// `DELETE /content/{id}/restriction/byOperation/{operation}/user?accountId=`.
pub async fn user_remove(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation)): Path<(String, String)>,
    params: Params,
) -> WikiResult<StatusCode> {
    subject_remove(&state, &user, &id, &operation, Subject::User(params)).await
}

/// `GET /content/{id}/restriction/byOperation/{operation}/group/{name}`, and `byGroupId/{id}`.
pub async fn group_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation, name)): Path<(String, String, String)>,
) -> WikiResult<StatusCode> {
    subject_status(&state, &user, &id, &operation, Subject::Group(name)).await
}

/// `PUT /content/{id}/restriction/byOperation/{operation}/group/{name}`, and `byGroupId/{id}`.
pub async fn group_add(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation, name)): Path<(String, String, String)>,
) -> WikiResult<StatusCode> {
    subject_add(&state, &user, &id, &operation, Subject::Group(name)).await
}

/// `DELETE /content/{id}/restriction/byOperation/{operation}/group/{name}`, and
/// `byGroupId/{id}`.
pub async fn group_remove(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, operation, name)): Path<(String, String, String)>,
) -> WikiResult<StatusCode> {
    subject_remove(&state, &user, &id, &operation, Subject::Group(name)).await
}
