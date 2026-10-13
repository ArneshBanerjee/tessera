//! Tessera's account API, under `/rest/tessera/1.0`.
//!
//! Atlassian manages accounts outside its products (at id.atlassian.com and admin.atlassian.com),
//! so these endpoints have no Jira or Confluence counterpart. They cover what a self-hosted
//! site needs: API tokens, passwords, profiles, and user administration.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, put};
use serde::Deserialize;
use serde_json::{Value, json};

use super::extract::Json;
use crate::AppState;
use crate::auth::{self, AdminUser, CurrentUser, SESSION_COOKIE};
use crate::error::{ApiError, ApiResult};
use crate::store::users::{self, NewUser, User};
use crate::time::format;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
        .route("/api-tokens", get(list_tokens).post(create_token))
        .route("/api-tokens/{id}", delete(revoke_token))
        .route("/password", put(change_password))
        .route("/profile", put(update_profile))
        .route("/admin/users", get(list_users).post(create_user))
        .route("/admin/users/{account_id}", put(update_user))
}

/// Passwords must be at least this long.
const MIN_PASSWORD: usize = 8;

fn check_password(password: &str) -> ApiResult<()> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(ApiError::field(
            "password",
            format!("Passwords must be at least {MIN_PASSWORD} characters."),
        ));
    }
    Ok(())
}

/// A user as an administrator sees them, with their status and role.
fn admin_view(user: &User) -> Value {
    json!({
        "accountId": user.account_id,
        "emailAddress": user.email,
        "displayName": user.display_name,
        "timeZone": user.time_zone,
        "active": user.active,
        "admin": user.is_admin,
        "hasPassword": user.password_hash.is_some(),
    })
}

/// The signed-in user, with their role.
async fn me(CurrentUser(user): CurrentUser) -> Json<Value> {
    Json(admin_view(&user))
}

async fn list_tokens(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<Json<Value>> {
    let tokens = auth::api_tokens(&state.db, user.id).await?;
    Ok(Json(json!(
        tokens
            .iter()
            .map(|t| json!({
                "id": t.id.to_string(),
                "label": t.label,
                "created": format(t.created_at),
                "lastUsed": t.last_used_at.map(format),
            }))
            .collect::<Vec<_>>()
    )))
}

#[derive(Deserialize)]
struct TokenRequest {
    label: String,
}

/// Creates a token. The secret is in this response only; Tessera keeps just its hash.
async fn create_token(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<TokenRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let label = request.label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err(ApiError::field(
            "label",
            "A label of 1 to 100 characters is required.",
        ));
    }
    let secret = auth::create_api_token(&state.db, user.id, label).await?;
    let created = auth::api_tokens(&state.db, user.id)
        .await?
        .into_iter()
        .last()
        .expect("the token was just created");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": created.id.to_string(), "label": created.label, "token": secret })),
    ))
}

async fn revoke_token(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::not_found("No such token."))?;
    if !auth::revoke_api_token(&state.db, user.id, id).await? {
        return Err(ApiError::not_found("No such token."));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordRequest {
    current_password: Option<String>,
    new_password: String,
}

/// Changes the signed-in user's password. The current one is required when there is one.
/// Other browser sessions end; this one stays signed in.
async fn change_password(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Json(request): Json<PasswordRequest>,
) -> ApiResult<StatusCode> {
    if let Some(hash) = &user.password_hash {
        let current = request.current_password.as_deref().unwrap_or_default();
        if !auth::verify_password(current, hash) {
            return Err(ApiError::field(
                "currentPassword",
                "The current password is not correct.",
            ));
        }
    }
    check_password(&request.new_password)?;
    users::set_password_hash(
        &state.db,
        user.id,
        &auth::hash_password(&request.new_password),
    )
    .await?;
    auth::end_sessions(&state.db, user.id, auth::cookie(&headers, SESSION_COOKIE)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileRequest {
    display_name: Option<String>,
    time_zone: Option<String>,
}

async fn update_profile(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<ProfileRequest>,
) -> ApiResult<Json<Value>> {
    let name = request.display_name.unwrap_or(user.display_name.clone());
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::field("displayName", "A name is required."));
    }
    let zone = request.time_zone.unwrap_or(user.time_zone.clone());
    if jiff::tz::TimeZone::get(&zone).is_err() {
        return Err(ApiError::field(
            "timeZone",
            format!("Unknown time zone: {zone}"),
        ));
    }
    users::update_profile(&state.db, user.id, name, &zone).await?;
    let updated = users::by_id(&state.db, user.id)
        .await?
        .expect("the user exists");
    Ok(Json(admin_view(&updated)))
}

async fn list_users(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> ApiResult<Json<Value>> {
    let all = users::all(&state.db).await?;
    Ok(Json(json!(all.iter().map(admin_view).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUserRequest {
    email_address: String,
    display_name: Option<String>,
    password: String,
    #[serde(default)]
    admin: bool,
}

async fn create_user(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let email = request.email_address.trim();
    if !email.contains('@') {
        return Err(ApiError::field(
            "emailAddress",
            "You must specify a valid email address.",
        ));
    }
    if users::by_email(&state.db, email).await?.is_some() {
        return Err(ApiError::field(
            "emailAddress",
            "A user with that email address already exists.",
        ));
    }
    check_password(&request.password)?;
    let display_name = request
        .display_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| email.split('@').next().unwrap_or(email).to_string());
    let user = users::create(
        &state.db,
        NewUser {
            email,
            display_name: display_name.trim(),
            password_hash: Some(auth::hash_password(&request.password)),
            is_admin: request.admin,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(admin_view(&user))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateUserRequest {
    active: Option<bool>,
    admin: Option<bool>,
    display_name: Option<String>,
    /// Sets a new password, for a user who lost theirs.
    password: Option<String>,
}

/// Changes another user. The site always keeps at least one active administrator.
async fn update_user(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(account_id): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> ApiResult<Json<Value>> {
    let user = users::by_account_id(&state.db, &account_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "User with accountId '{account_id}' does not exist."
            ))
        })?;
    let active = request.active.unwrap_or(user.active);
    let admin = request.admin.unwrap_or(user.is_admin);
    let loses_admin = user.active && user.is_admin && !(active && admin);
    if loses_admin && users::active_admin_count(&state.db).await? <= 1 {
        return Err(ApiError::bad_request(
            "The site needs at least one active administrator.",
        ));
    }
    users::set_flags(&state.db, user.id, active, admin).await?;
    if let Some(name) = request
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        users::update_profile(&state.db, user.id, name, &user.time_zone).await?;
    }
    if let Some(password) = &request.password {
        check_password(password)?;
        users::set_password_hash(&state.db, user.id, &auth::hash_password(password)).await?;
        auth::end_sessions(&state.db, user.id, None).await?;
    }
    if !active {
        auth::end_sessions(&state.db, user.id, None).await?;
    }
    let updated = users::by_id(&state.db, user.id)
        .await?
        .expect("the user exists");
    Ok(Json(admin_view(&updated)))
}
