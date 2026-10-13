//! Users: `/myself`, `/user`, `/user/search`, `/users/search`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;

use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser, hash_password};
use crate::error::{ApiError, ApiResult};
use crate::store::users::{self, NewUser};

pub async fn myself(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.user(&user)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuery {
    account_id: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<UserQuery>,
) -> ApiResult<Json<Value>> {
    let account_id = query.account_id.ok_or_else(|| {
        ApiError::bad_request("The accountId query parameter needs to be provided.")
    })?;
    let user = users::by_account_id(&state.db, &account_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "User with accountId '{account_id}' does not exist."
            ))
        })?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.user(&user)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewUserRequest {
    email_address: String,
    display_name: Option<String>,
    /// Tessera extension: Jira Cloud invites users by email instead.
    password: Option<String>,
}

/// Creates a user. Site administrators only.
pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<NewUserRequest>,
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
    let display_name = request
        .display_name
        .unwrap_or_else(|| email.split('@').next().unwrap_or(email).to_string());
    let user = users::create(
        &state.db,
        NewUser {
            email,
            display_name: &display_name,
            password_hash: request.password.as_deref().map(hash_password),
            is_admin: false,
        },
    )
    .await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok((StatusCode::CREATED, Json(ctx.user(&user))))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    query: Option<String>,
    /// Older clients search by `username`.
    username: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

/// Finds users by name or email. Returns a plain array, like Jira.
pub async fn search(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<SearchQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Vec<Value>>> {
    let text = query
        .query
        .or(query.username)
        .or(query.account_id)
        .unwrap_or_default();
    let (start_at, max_results) = page.bounded(1000);
    let found = users::search(&state.db, &text, start_at, max_results).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(found.iter().map(|user| ctx.user(user)).collect()))
}

/// Lists all users.
pub async fn list(
    state: State<AppState>,
    version: Extension<ApiVersion>,
    user: CurrentUser,
    page: Query<Page>,
) -> ApiResult<Json<Vec<Value>>> {
    let everyone = Query(SearchQuery {
        query: None,
        username: None,
        account_id: None,
    });
    search(state, version, user, everyone, page).await
}
