//! `/rest/auth/1/session`: cookie sessions for the web app, in the shape Jira Server uses.

use axum::Json;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth::{self, CurrentUser, SESSION_COOKIE};
use crate::error::{ApiError, ApiResult};

#[derive(Deserialize)]
pub struct Credentials {
    /// Jira Server calls this `username`; Tessera signs in by email.
    username: String,
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> ApiResult<impl IntoResponse> {
    let user = auth::check_credentials(&state.db, &credentials.username, &credentials.password)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "Login failed. Check your email and password.",
            )
        })?;
    let secret = auth::create_session(&state.db, user.id).await?;
    let cookie =
        format!("{SESSION_COOKIE}={secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000");
    Ok((
        [(SET_COOKIE, cookie)],
        Json(json!({ "session": { "name": SESSION_COOKIE, "value": secret } })),
    ))
}

pub async fn current(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> impl IntoResponse {
    Json(json!({
        "self": format!("{}/rest/api/3/user?accountId={}", state.base_url, user.account_id),
        "name": user.email,
        "accountId": user.account_id,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    if let Some(secret) = auth::cookie(&headers, SESSION_COOKIE) {
        auth::delete_session(&state.db, secret).await?;
    }
    let expired = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, expired)]))
}
