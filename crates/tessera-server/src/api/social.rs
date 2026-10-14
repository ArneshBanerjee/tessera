//! Watchers and votes: `/issue/{idOrKey}/watchers` and `/issue/{idOrKey}/votes`.

use super::extract::Json;
use crate::api::access::{find_issue_access, find_issue_as};
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::issues::Issue;
use crate::store::social::{self, List};
use crate::store::users;

/// The `watches` field of an issue: count and whether the viewer watches.
pub async fn watches_json(ctx: &Ctx, issue: &Issue) -> ApiResult<Value> {
    let watchers = social::members(&ctx.state.db, List::Watchers, issue.id).await?;
    Ok(json!({
        "self": ctx.api_url(&format!("/issue/{}/watchers", issue.key)),
        "watchCount": watchers.len(),
        "isWatching": ctx.viewer.is_some_and(|v| watchers.contains(&v)),
    }))
}

/// The `votes` field of an issue: count and whether the viewer voted.
pub async fn votes_json(ctx: &Ctx, issue: &Issue) -> ApiResult<Value> {
    let voters = social::members(&ctx.state.db, List::Votes, issue.id).await?;
    Ok(json!({
        "self": ctx.api_url(&format!("/issue/{}/votes", issue.key)),
        "votes": voters.len(),
        "hasVoted": ctx.viewer.is_some_and(|v| voters.contains(&v)),
    }))
}

async fn people(ctx: &mut Ctx, ids: &[i64]) -> ApiResult<Vec<Value>> {
    let mut rendered = Vec::with_capacity(ids.len());
    for id in ids {
        let user = ctx.user_ref(Some(*id)).await?;
        if !user.is_null() {
            rendered.push(user);
        }
    }
    Ok(rendered)
}

pub async fn watchers(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "VIEW_VOTERS_AND_WATCHERS").await?;
    let mut ctx = Ctx::new(&state, version).await?.viewed_by(user.id);
    let mut value = watches_json(&ctx, &issue).await?;
    let ids = social::members(&state.db, List::Watchers, issue.id).await?;
    value["watchers"] = json!(people(&mut ctx, &ids).await?);
    Ok(Json(value))
}

/// Adds a watcher. The body is a JSON string with an account id; an empty body means the
/// caller. Adding someone else needs the Manage Watchers permission.
pub async fn add_watcher(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let (issue, access) = find_issue_access(&state, &user, &id_or_key).await?;
    let target = if body.iter().all(u8::is_ascii_whitespace) {
        user.clone()
    } else {
        let account_id: String = serde_json::from_slice(&body).map_err(|_| {
            ApiError::bad_request("The body must be a JSON string with an account id.")
        })?;
        users::by_account_id(&state.db, &account_id)
            .await?
            .filter(|u| u.active)
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "The user with account id {account_id} does not exist."
                ))
            })?
    };
    // Anyone may watch; only holders of Manage Watchers change the list for others.
    if target.id != user.id && !access.can_on("MANAGE_WATCHERS", Some(&issue)) {
        return Err(ApiError::forbidden(
            "You do not have the permission to manage the watcher list.",
        ));
    }
    social::add(&state.db, List::Watchers, issue.id, target.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherQuery {
    account_id: Option<String>,
}

pub async fn remove_watcher(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(query): Query<WatcherQuery>,
) -> ApiResult<StatusCode> {
    let (issue, access) = find_issue_access(&state, &user, &id_or_key).await?;
    let target = match query.account_id {
        Some(account_id) => users::by_account_id(&state.db, &account_id)
            .await?
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "The user with account id {account_id} does not exist."
                ))
            })?,
        None => user.clone(),
    };
    // Anyone may watch; only holders of Manage Watchers change the list for others.
    if target.id != user.id && !access.can_on("MANAGE_WATCHERS", Some(&issue)) {
        return Err(ApiError::forbidden(
            "You do not have the permission to manage the watcher list.",
        ));
    }
    social::remove(&state.db, List::Watchers, issue.id, target.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn votes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "VIEW_VOTERS_AND_WATCHERS").await?;
    let mut ctx = Ctx::new(&state, version).await?.viewed_by(user.id);
    let mut value = votes_json(&ctx, &issue).await?;
    let ids = social::members(&state.db, List::Votes, issue.id).await?;
    value["voters"] = json!(people(&mut ctx, &ids).await?);
    Ok(Json(value))
}

/// Votes for an issue as the caller. As in Jira, reporters cannot vote for their own issues,
/// and nobody can vote for a resolved issue.
pub async fn add_vote(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    if issue.reporter_id == Some(user.id) {
        return Err(ApiError::bad_request(
            "You cannot vote for an issue you have reported.",
        ));
    }
    if issue.resolution_id.is_some() {
        return Err(ApiError::bad_request(
            "You cannot vote for an issue that has been resolved.",
        ));
    }
    social::add(&state.db, List::Votes, issue.id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_vote(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    social::remove(&state.db, List::Votes, issue.id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
