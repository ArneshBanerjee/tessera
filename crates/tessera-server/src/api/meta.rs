//! Issue metadata: issue types, statuses, status categories, priorities, and resolutions.

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde_json::Value;

use super::ApiVersion;
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};

async fn ctx(state: &AppState, version: ApiVersion) -> ApiResult<Ctx> {
    Ctx::new(state, version).await
}

pub async fn issue_types(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = ctx(&state, version).await?;
    Ok(Json(
        ctx.catalog
            .issue_types
            .iter()
            .map(|t| ctx.issue_type(t))
            .collect(),
    ))
}

pub async fn issue_type(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, version).await?;
    let issue_type = ctx
        .catalog
        .issue_type(id)
        .ok_or_else(|| ApiError::not_found(format!("Issue type with ID {id} not found.")))?;
    Ok(Json(ctx.issue_type(issue_type)))
}

pub async fn statuses(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = ctx(&state, version).await?;
    Ok(Json(
        ctx.catalog.statuses.iter().map(|s| ctx.status(s)).collect(),
    ))
}

/// Jira accepts a status id or a status name here.
pub async fn status(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id_or_name): Path<String>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, version).await?;
    let status = ctx
        .catalog
        .statuses
        .iter()
        .find(|s| s.id.to_string() == id_or_name || s.name.eq_ignore_ascii_case(&id_or_name))
        .ok_or_else(|| {
            ApiError::not_found(format!("The status with id '{id_or_name}' does not exist"))
        })?;
    Ok(Json(ctx.status(status)))
}

pub async fn status_categories(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = ctx(&state, version).await?;
    Ok(Json(
        ctx.catalog
            .status_categories
            .iter()
            .map(|c| ctx.status_category(c))
            .collect(),
    ))
}

pub async fn priorities(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = ctx(&state, version).await?;
    Ok(Json(
        ctx.catalog
            .priorities
            .iter()
            .map(|p| ctx.priority(p))
            .collect(),
    ))
}

pub async fn priority(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, version).await?;
    let priority = ctx
        .catalog
        .priority(id)
        .ok_or_else(|| ApiError::not_found(format!("Priority with ID {id} not found.")))?;
    Ok(Json(ctx.priority(priority)))
}

pub async fn resolutions(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = ctx(&state, version).await?;
    Ok(Json(
        ctx.catalog
            .resolutions
            .iter()
            .map(|r| ctx.resolution(r))
            .collect(),
    ))
}

pub async fn resolution(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx(&state, version).await?;
    let resolution = ctx
        .catalog
        .resolution(id)
        .ok_or_else(|| ApiError::not_found(format!("Resolution with ID {id} not found.")))?;
    Ok(Json(ctx.resolution(resolution)))
}
