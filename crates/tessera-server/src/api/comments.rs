//! Comments: `/issue/{idOrKey}/comment` and `/issue/{idOrKey}/comment/{id}`.

use super::extract::Json;
use crate::api::access::{check, find_issue_access, find_issue_as};
use crate::notify::{self, IssueEvent};
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::issues::comment_json;
use super::render::{Ctx, rich_text_input};
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::comments::{self, Comment};
use crate::store::social::{self, List};

#[derive(Deserialize)]
pub struct CommentRequest {
    body: Value,
}

fn parse_body(body: &Value) -> ApiResult<tessera_adf::Node> {
    let stored = rich_text_input("comment", body)?
        .ok_or_else(|| ApiError::field("comment", "Comment body can not be empty!"))?;
    Ok(serde_json::from_str(&stored).expect("stored ADF parses"))
}

async fn find_comment(state: &AppState, issue_id: i64, id: &str) -> ApiResult<Comment> {
    let not_found = || ApiError::not_found(format!("Can not find a comment for the id: {id}."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    comments::by_id(&state.db, issue_id, id)
        .await?
        .ok_or_else(not_found)
}

pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let (start_at, max_results) = page.bounded(5000);
    let (page, total) = comments::list(&state.db, issue.id, start_at, max_results).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let mut rendered = Vec::with_capacity(page.len());
    for comment in &page {
        rendered.push(comment_json(&mut ctx, issue.id, comment).await?);
    }
    Ok(Json(json!({
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "comments": rendered,
    })))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<CommentRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let issue = find_issue_as(&state, &user, &id_or_key, "ADD_COMMENTS").await?;
    let body = parse_body(&request.body)?;
    let comment = comments::create(&state.db, issue.id, user.id, &body).await?;
    // Commenting makes you a watcher, as with Jira's default autowatch setting.
    social::add(&state.db, List::Watchers, issue.id, user.id).await?;
    let text = tessera_adf::to_plain_text(&body);
    let mentioned = tessera_adf::mentions(&body);
    notify::issue_with_mentions(
        &state,
        &user,
        &issue,
        IssueEvent::Commented(&text),
        &mentioned,
    )
    .await;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(comment_json(&mut ctx, issue.id, &comment).await?),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(comment_json(&mut ctx, issue.id, &comment).await?))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
    Json(request): Json<CommentRequest>,
) -> ApiResult<Json<Value>> {
    let (issue, access) = find_issue_access(&state, &user, &id_or_key).await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    let own = comment.author_id == user.id;
    check(
        access.can_own_or_all("EDIT_OWN_COMMENTS", "EDIT_ALL_COMMENTS", own, &issue),
        "EDIT_ALL_COMMENTS",
    )?;
    let body = parse_body(&request.body)?;
    let updated = comments::update(&state.db, comment.id, user.id, &body).await?;
    // People newly mentioned in the edit hear about it.
    let before = serde_json::from_str::<tessera_adf::Node>(&comment.body)
        .map(|doc| tessera_adf::mentions(&doc))
        .unwrap_or_default();
    let mentioned: Vec<String> = tessera_adf::mentions(&body)
        .into_iter()
        .filter(|id| !before.contains(id))
        .collect();
    let text = tessera_adf::to_plain_text(&body);
    notify::issue_mentions(&state, &user, &issue, &text, &mentioned).await;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(comment_json(&mut ctx, issue.id, &updated).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let (issue, access) = find_issue_access(&state, &user, &id_or_key).await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    let own = comment.author_id == user.id;
    check(
        access.can_own_or_all("DELETE_OWN_COMMENTS", "DELETE_ALL_COMMENTS", own, &issue),
        "DELETE_ALL_COMMENTS",
    )?;
    comments::delete(&state.db, comment.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
