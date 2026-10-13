//! Comments: `/issue/{idOrKey}/comment` and `/issue/{idOrKey}/comment/{id}`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};

use super::issues::{comment_json, find_issue};
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
    CurrentUser(_): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
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
    let issue = find_issue(&state, &id_or_key).await?;
    let comment =
        comments::create(&state.db, issue.id, user.id, &parse_body(&request.body)?).await?;
    // Commenting makes you a watcher, as with Jira's default autowatch setting.
    social::add(&state.db, List::Watchers, issue.id, user.id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(comment_json(&mut ctx, issue.id, &comment).await?),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(comment_json(&mut ctx, issue.id, &comment).await?))
}

/// Only the author or a site administrator may change or delete a comment, as in Jira.
fn check_owner(user: &crate::store::users::User, comment: &Comment) -> ApiResult<()> {
    if comment.author_id == user.id || user.is_admin {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "You do not have the permission to edit this comment.",
        ))
    }
}

pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
    Json(request): Json<CommentRequest>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    check_owner(&user, &comment)?;
    let updated =
        comments::update(&state.db, comment.id, user.id, &parse_body(&request.body)?).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(comment_json(&mut ctx, issue.id, &updated).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let issue = find_issue(&state, &id_or_key).await?;
    let comment = find_comment(&state, issue.id, &id).await?;
    check_owner(&user, &comment)?;
    comments::delete(&state.db, comment.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
