//! In-app notifications: `/rest/tessera/1.0/notifications`. Jira and Confluence keep theirs
//! behind private APIs, so these are Tessera's own.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use super::extract::Json;
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::notifications;
use crate::time::format;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/read-all", post(read_all))
        .route(
            "/notifications/{id}/read",
            post(mark_read).delete(mark_unread),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(default)]
    start_at: i64,
    max_results: Option<i64>,
    #[serde(default)]
    unread_only: bool,
}

/// `GET /notifications`: the caller's notifications, newest first, with the unread count.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.max_results.unwrap_or(30).clamp(0, 200);
    let found = notifications::of_user(
        &state.db,
        user.id,
        query.unread_only,
        query.start_at.max(0),
        limit,
    )
    .await?;
    let mut ctx = Ctx::new(&state, ApiVersion::V3).await?;
    let mut values = Vec::with_capacity(found.len());
    for note in &found {
        values.push(json!({
            "id": note.id.to_string(),
            "kind": note.kind,
            "title": note.title,
            "body": note.body,
            "link": note.link,
            "actor": ctx.user_ref(note.actor_id).await?,
            "created": format(note.created_at),
            "read": note.read_at.is_some(),
        }));
    }
    Ok(Json(json!({
        "startAt": query.start_at.max(0),
        "maxResults": limit,
        "unread": notifications::unread_count(&state.db, user.id).await?,
        "values": values,
    })))
}

async fn set_read(state: &AppState, user_id: i64, id: i64, read: bool) -> ApiResult<StatusCode> {
    if !notifications::set_read(&state.db, user_id, id, read).await? {
        return Err(ApiError::not_found("No such notification."));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /notifications/{id}/read`.
pub async fn mark_read(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    set_read(&state, user.id, id, true).await
}

/// `DELETE /notifications/{id}/read`: marks it unread again.
pub async fn mark_unread(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    set_read(&state, user.id, id, false).await
}

/// `POST /notifications/read-all`.
pub async fn read_all(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<StatusCode> {
    notifications::read_all(&state.db, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
