//! Watching pages and spaces: `/user/watch/content/{id}` and `/user/watch/space/{key}`.
//!
//! Without `accountId`, the caller watches. Site administrators may name another user.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::Params;
use super::error::{WikiError, WikiResult};
use super::service::find_current;
use super::spaces::find_space;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::users::{self, User};
use crate::store::wiki_watchers::{self, Target};

/// The user a watch request is about: the caller, or (for administrators) `accountId`.
async fn subject(state: &AppState, caller: &User, params: &Params) -> WikiResult<User> {
    match params.get("accountId") {
        None => Ok(caller.clone()),
        Some(id) if id == caller.account_id => Ok(caller.clone()),
        Some(_) if !caller.is_admin => Err(WikiError::new(
            StatusCode::FORBIDDEN,
            "Only administrators may change what other users watch",
        )),
        Some(id) => users::by_account_id(&state.db, id)
            .await?
            .ok_or_else(|| WikiError::bad_request(format!("No user with account id {id}"))),
    }
}

async fn content_target(state: &AppState, user: &User, id: &str) -> WikiResult<Target> {
    Ok(Target::Content(find_current(state, user, id).await?.id))
}

async fn space_target(state: &AppState, user: &User, key: &str) -> WikiResult<Target> {
    Ok(Target::Space(find_space(state, user, key).await?.id))
}

pub async fn content_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let target = content_target(&state, &user, &id).await?;
    let who = subject(&state, &user, &params).await?;
    Ok(Json(
        json!({ "watching": wiki_watchers::is_watching(&state.db, target, who.id).await? }),
    ))
}

pub async fn watch_content(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let target = content_target(&state, &user, &id).await?;
    let who = subject(&state, &user, &params).await?;
    wiki_watchers::watch(&state.db, target, who.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unwatch_content(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let target = content_target(&state, &user, &id).await?;
    let who = subject(&state, &user, &params).await?;
    wiki_watchers::unwatch(&state.db, target, who.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn space_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let target = space_target(&state, &user, &key).await?;
    let who = subject(&state, &user, &params).await?;
    Ok(Json(
        json!({ "watching": wiki_watchers::is_watching(&state.db, target, who.id).await? }),
    ))
}

pub async fn watch_space(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let target = space_target(&state, &user, &key).await?;
    let who = subject(&state, &user, &params).await?;
    wiki_watchers::watch(&state.db, target, who.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unwatch_space(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    params: Params,
) -> WikiResult<StatusCode> {
    let target = space_target(&state, &user, &key).await?;
    let who = subject(&state, &user, &params).await?;
    wiki_watchers::unwatch(&state.db, target, who.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
