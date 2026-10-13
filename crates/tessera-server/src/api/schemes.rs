//! Schemes and project categories.
//!
//! Tessera has one permission model (signed-in users browse and edit; site administrators
//! administer), one notification setup, and no issue security levels yet. Each is exposed as
//! the single default scheme Jira clients look for when they create projects.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use super::render::Ctx;
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::projects::{self, Category};

/// The id of the default permission scheme in Jira.
pub const DEFAULT_PERMISSION_SCHEME: i64 = 0;
pub const DEFAULT_NOTIFICATION_SCHEME: i64 = 10000;
pub const DEFAULT_SECURITY_SCHEME: i64 = 10000;

fn permission_scheme(ctx: &Ctx) -> Value {
    json!({
        "expand": "permissions,user,group,projectRole,field,all",
        "id": DEFAULT_PERMISSION_SCHEME,
        "self": ctx.api_url(&format!("/permissionscheme/{DEFAULT_PERMISSION_SCHEME}")),
        "name": "Default Permission Scheme",
        "description": "Signed-in users can browse and edit all projects. Site administrators administer them.",
    })
}

pub async fn permission_schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(
        json!({ "permissionSchemes": [permission_scheme(&ctx)] }),
    ))
}

pub async fn permission_scheme_by_id(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    if id != DEFAULT_PERMISSION_SCHEME {
        return Err(ApiError::not_found(format!(
            "Permission scheme {id} does not exist."
        )));
    }
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(permission_scheme(&ctx)))
}

pub async fn issue_security_schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "issueSecuritySchemes": [{
            "self": ctx.api_url(&format!("/issuesecurityschemes/{DEFAULT_SECURITY_SCHEME}")),
            "id": DEFAULT_SECURITY_SCHEME,
            "name": "Default",
            "description": "No security levels: everyone who can see a project sees all of its issues.",
        }]
    })))
}

pub async fn notification_schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "maxResults": 50,
        "startAt": 0,
        "total": 1,
        "isLast": true,
        "values": [{
            "expand": "notificationSchemeEvents,user,group,projectRole,field,all",
            "id": DEFAULT_NOTIFICATION_SCHEME,
            "self": ctx.api_url(&format!("/notificationscheme/{DEFAULT_NOTIFICATION_SCHEME}")),
            "name": "Default Notification Scheme",
        }]
    })))
}

pub fn category_json(ctx: &Ctx, category: &Category) -> Value {
    json!({
        "self": ctx.api_url(&format!("/projectCategory/{}", category.id)),
        "id": category.id.to_string(),
        "name": category.name,
        "description": category.description,
    })
}

pub async fn categories(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = projects::categories(&state.db).await?;
    Ok(Json(all.iter().map(|c| category_json(&ctx, c)).collect()))
}

async fn find_category(state: &AppState, id: i64) -> ApiResult<Category> {
    projects::category(&state.db, id).await?.ok_or_else(|| {
        ApiError::not_found(format!("The project category with id {id} does not exist."))
    })
}

pub async fn category(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let category = find_category(&state, id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(category_json(&ctx, &category)))
}

#[derive(Deserialize)]
pub struct CategoryRequest {
    name: Option<String>,
    description: Option<String>,
}

/// Creates a project category. Site administrators only.
pub async fn create_category(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<CategoryRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request.name.as_deref().map(str::trim).unwrap_or_default();
    if name.is_empty() {
        return Err(ApiError::field(
            "name",
            "The category name must not be empty.",
        ));
    }
    let exists = projects::categories(&state.db)
        .await?
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name));
    if exists {
        return Err(ApiError::field(
            "name",
            format!("The project category '{name}' already exists."),
        ));
    }
    let category = projects::create_category(
        &state.db,
        name,
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok((StatusCode::CREATED, Json(category_json(&ctx, &category))))
}

/// Updates a project category. Site administrators only.
pub async fn update_category(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<CategoryRequest>,
) -> ApiResult<Json<Value>> {
    find_category(&state, id).await?;
    projects::update_category(
        &state.db,
        id,
        request.name.as_deref(),
        request.description.as_deref(),
    )
    .await?;
    let category = find_category(&state, id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(category_json(&ctx, &category)))
}

/// Deletes a project category. Its projects keep existing, without a category.
pub async fn delete_category(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_category(&state, id).await?;
    projects::delete_category(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
