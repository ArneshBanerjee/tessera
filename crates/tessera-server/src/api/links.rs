//! Issue links and link types: `/issueLink` and `/issueLinkType`.
//!
//! A link reads "outward issue <outward description> inward issue", for example
//! "PROJ-1 blocks PROJ-2" when PROJ-1 is the outward issue of a Blocks link. In an issue's own
//! `issuelinks` field, Jira names the *other* issue: PROJ-1 shows `outwardIssue: PROJ-2`
//! ("blocks PROJ-2"), and PROJ-2 shows `inwardIssue: PROJ-1` ("is blocked by PROJ-1").

use super::extract::Json;
use crate::api::access::require_issue;
use crate::store::users::User;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use super::render::{Ctx, rich_text_input};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::comments;
use crate::store::issues::{self, Issue};
use crate::store::links::{self, Link, LinkType};
use crate::store::meta::json_id;

/// A linked issue with a few fields, as Jira nests it inside links.
fn linked_issue(ctx: &Ctx, issue: &Issue) -> Value {
    let catalog = &ctx.catalog;
    json!({
        "id": issue.id.to_string(),
        "key": issue.key,
        "self": ctx.api_url(&format!("/issue/{}", issue.id)),
        "fields": {
            "summary": issue.summary,
            "status": catalog.status(issue.status_id).map(|s| ctx.status(s)),
            "priority": issue.priority_id.and_then(|id| catalog.priority(id)).map(|p| ctx.priority(p)),
            "issuetype": catalog.issue_type(issue.issue_type_id).map(|t| ctx.issue_type(t)),
        },
    })
}

/// The `issuelinks` field of an issue.
pub async fn issue_links_json(ctx: &Ctx, issue: &Issue) -> ApiResult<Value> {
    let db = &ctx.state.db;
    let types = links::types(db).await?;
    let mut rendered = Vec::new();
    for link in links::of_issue(db, issue.id).await? {
        let Some(link_type) = types.iter().find(|t| t.id == link.link_type_id) else {
            continue;
        };
        let (side, other_id) = if link.outward_issue_id == issue.id {
            ("outwardIssue", link.inward_issue_id)
        } else {
            ("inwardIssue", link.outward_issue_id)
        };
        let Some(other) = issues::by_id(db, other_id).await? else {
            continue;
        };
        rendered.push(json!({
            "id": link.id.to_string(),
            "self": ctx.api_url(&format!("/issueLink/{}", link.id)),
            "type": ctx.link_type_json(link_type),
            side: linked_issue(ctx, &other),
        }));
    }
    Ok(Value::Array(rendered))
}

async fn find_type(state: &AppState, id: &str) -> ApiResult<LinkType> {
    let not_found = || ApiError::not_found(format!("No issue link type with id '{id}' found."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    links::type_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)
}

/// Finds a link type by `{"id": ...}` or `{"name": ...}`.
async fn type_by_ref(state: &AppState, reference: &Value) -> ApiResult<LinkType> {
    let all = links::types(&state.db).await?;
    let found = match (
        reference.get("id").and_then(json_id),
        reference.get("name").and_then(Value::as_str),
    ) {
        (Some(id), _) => all.into_iter().find(|t| t.id == id),
        (None, Some(name)) => all.into_iter().find(|t| t.name.eq_ignore_ascii_case(name)),
        _ => None,
    };
    found.ok_or_else(|| ApiError::not_found("No issue link type with name or id found."))
}

async fn issue_by_ref(state: &AppState, reference: &Value, field: &str) -> ApiResult<Issue> {
    let id_or_key = reference
        .get("key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            reference
                .get("id")
                .and_then(json_id)
                .map(|id| id.to_string())
        })
        .ok_or_else(|| ApiError::bad_request(format!("The {field} is required.")))?;
    issues::by_id_or_key(&state.db, &id_or_key)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Issue Does Not Exist: {id_or_key}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRequest {
    #[serde(rename = "type")]
    link_type: Value,
    inward_issue: Value,
    outward_issue: Value,
    comment: Option<Value>,
}

/// Creates a link. A comment in the request goes on the outward (from) issue. Creating a link
/// that already exists succeeds without a duplicate, as in Jira.
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<LinkRequest>,
) -> ApiResult<StatusCode> {
    let link_type = type_by_ref(&state, &request.link_type).await?;
    let inward = issue_by_ref(&state, &request.inward_issue, "inwardIssue").await?;
    let outward = issue_by_ref(&state, &request.outward_issue, "outwardIssue").await?;
    // Linking needs Link Issues on the issue the link starts from, and sight of the other.
    require_issue(&state, &user, &outward, "LINK_ISSUES").await?;
    require_issue(&state, &user, &inward, "BROWSE_PROJECTS").await?;
    if inward.id == outward.id {
        return Err(ApiError::bad_request("You cannot link an issue to itself."));
    }
    links::create(&state.db, link_type.id, outward.id, inward.id).await?;
    if let Some(comment) = request.comment {
        let body = comment.get("body").unwrap_or(&Value::Null);
        if let Some(stored) = rich_text_input("comment", body)? {
            let node = serde_json::from_str(&stored).expect("stored ADF parses");
            comments::create(&state.db, outward.id, user.id, &node).await?;
        }
    }
    Ok(StatusCode::CREATED)
}

/// A link between two issues the user can see.
async fn find_link(state: &AppState, user: &User, id: &str) -> ApiResult<Link> {
    let not_found = || ApiError::not_found(format!("No issue link with id '{id}' exists."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    let link = links::by_id(&state.db, id).await?.ok_or_else(not_found)?;
    for issue_id in [link.outward_issue_id, link.inward_issue_id] {
        let issue = issues::by_id(&state.db, issue_id)
            .await?
            .ok_or_else(not_found)?;
        require_issue(state, user, &issue, "BROWSE_PROJECTS")
            .await
            .map_err(|_| not_found())?;
    }
    Ok(link)
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let link = find_link(&state, &user, &id).await?;
    let ctx = Ctx::new(&state, version).await?;
    let link_type = links::type_by_id(&state.db, link.link_type_id)
        .await?
        .expect("link has a type");
    let outward = issues::by_id(&state.db, link.outward_issue_id)
        .await?
        .expect("linked issue exists");
    let inward = issues::by_id(&state.db, link.inward_issue_id)
        .await?
        .expect("linked issue exists");
    Ok(Json(json!({
        "id": link.id.to_string(),
        "self": ctx.api_url(&format!("/issueLink/{}", link.id)),
        "type": ctx.link_type_json(&link_type),
        "inwardIssue": linked_issue(&ctx, &inward),
        "outwardIssue": linked_issue(&ctx, &outward),
    })))
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let link = find_link(&state, &user, &id).await?;
    let outward = issues::by_id(&state.db, link.outward_issue_id)
        .await?
        .expect("linked issue exists");
    require_issue(&state, &user, &outward, "LINK_ISSUES").await?;
    links::delete(&state.db, link.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn types(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = links::types(&state.db).await?;
    Ok(Json(json!({
        "issueLinkTypes": all.iter().map(|t| ctx.link_type_json(t)).collect::<Vec<_>>(),
    })))
}

pub async fn get_type(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let link_type = find_type(&state, &id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.link_type_json(&link_type)))
}

#[derive(Deserialize)]
pub struct LinkTypeRequest {
    name: Option<String>,
    inward: Option<String>,
    outward: Option<String>,
}

fn required(value: Option<String>, field: &str) -> ApiResult<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::field(field, format!("The {field} must not be empty.")))
}

async fn check_type_name(state: &AppState, name: &str, except: Option<i64>) -> ApiResult<()> {
    let taken = links::types(&state.db)
        .await?
        .iter()
        .any(|t| t.name.eq_ignore_ascii_case(name) && Some(t.id) != except);
    if taken {
        return Err(ApiError::field(
            "name",
            format!("An issue link type with the name '{name}' already exists."),
        ));
    }
    Ok(())
}

/// Creates a link type. Site administrators only.
pub async fn create_type(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<LinkTypeRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = required(request.name, "name")?;
    let inward = required(request.inward, "inward")?;
    let outward = required(request.outward, "outward")?;
    check_type_name(&state, &name, None).await?;
    let link_type = links::create_type(&state.db, &name, &inward, &outward).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok((StatusCode::CREATED, Json(ctx.link_type_json(&link_type))))
}

/// Updates a link type. Site administrators only.
pub async fn update_type(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
    Json(request): Json<LinkTypeRequest>,
) -> ApiResult<Json<Value>> {
    let mut link_type = find_type(&state, &id).await?;
    if let Some(name) = request.name {
        link_type.name = required(Some(name), "name")?;
        check_type_name(&state, &link_type.name, Some(link_type.id)).await?;
    }
    if let Some(inward) = request.inward {
        link_type.inward = required(Some(inward), "inward")?;
    }
    if let Some(outward) = request.outward {
        link_type.outward = required(Some(outward), "outward")?;
    }
    links::update_type(&state.db, &link_type).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.link_type_json(&link_type)))
}

/// Deletes a link type and every link of that type. Site administrators only.
pub async fn delete_type(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let link_type = find_type(&state, &id).await?;
    links::delete_type(&state.db, link_type.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
