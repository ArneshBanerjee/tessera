//! Attachments: upload to an issue, read metadata, download, and delete.

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Value, json};

use super::ApiVersion;
use super::issues::find_issue;
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::attachments::{self, Attachment, SaveError};

/// The largest file Tessera accepts, in bytes (100 MiB).
pub const UPLOAD_LIMIT: usize = 100 * 1024 * 1024;

impl From<SaveError> for ApiError {
    fn from(error: SaveError) -> Self {
        match error {
            SaveError::Database(error) => error.into(),
            SaveError::File(error) => {
                tracing::error!(%error, "could not write attachment");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not save the attachment.",
                )
            }
        }
    }
}

/// `GET /attachment/meta`.
pub async fn meta(CurrentUser(_): CurrentUser) -> Json<Value> {
    Json(json!({ "enabled": true, "uploadLimit": UPLOAD_LIMIT }))
}

/// Uploads one or more files in a multipart request, each in a part named `file`.
///
/// Like Jira, the request must carry `X-Atlassian-Token: no-check`. This header protects
/// against cross-site request forgery: a browser form on another site cannot set it.
pub async fn upload(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Json<Vec<Value>>> {
    let token = headers
        .get("X-Atlassian-Token")
        .and_then(|v| v.to_str().ok());
    if !token.is_some_and(|t| t.eq_ignore_ascii_case("no-check")) {
        return Err(ApiError::forbidden("XSRF check failed"));
    }
    let issue = find_issue(&state, &id_or_key).await?;
    let mut saved: Vec<Attachment> = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Could not read the upload: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("attachment").to_string();
        let mime_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::bad_request(format!("Could not read the upload: {e}")))?;
        saved.push(
            attachments::create(
                &state.db,
                &state.data_dir,
                issue.id,
                user.id,
                &filename,
                &mime_type,
                &bytes,
            )
            .await?,
        );
    }
    if saved.is_empty() {
        return Err(ApiError::bad_request(
            "The request must contain a file in a part named 'file'.",
        ));
    }
    let mut ctx = Ctx::new(&state, version).await?;
    let mut rendered = Vec::with_capacity(saved.len());
    for attachment in &saved {
        rendered.push(ctx.attachment_json(attachment).await?);
    }
    Ok(Json(rendered))
}

async fn find(state: &AppState, id: &str) -> ApiResult<Attachment> {
    let not_found =
        || ApiError::not_found(format!("The attachment with id '{id}' does not exist."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    attachments::by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let attachment = find(&state, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.attachment_json(&attachment).await?))
}

/// Downloads the file contents.
pub async fn content(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let attachment = find(&state, &id).await?;
    let bytes = tokio::fs::read(attachments::path(&state.data_dir, attachment.id))
        .await
        .map_err(|_| ApiError::not_found("The attachment file is missing."))?;
    // Quotes and backslashes would break the header, so they become underscores.
    let safe_name: String = attachment
        .filename
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    Ok((
        [
            (header::CONTENT_TYPE, attachment.mime_type.clone()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe_name}\""),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// Deletes an attachment. Only its author or a site administrator may.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let attachment = find(&state, &id).await?;
    if attachment.author_id != user.id && !user.is_admin {
        return Err(ApiError::forbidden(
            "You do not have permission to delete this attachment.",
        ));
    }
    attachments::delete(&state.db, &state.data_dir, attachment.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The `attachment` field of an issue.
pub async fn issue_attachments_json(ctx: &mut Ctx, issue_id: i64) -> ApiResult<Value> {
    let all = attachments::of_issue(&ctx.state.db, issue_id).await?;
    let mut rendered = Vec::with_capacity(all.len());
    for attachment in &all {
        rendered.push(ctx.attachment_json(attachment).await?);
    }
    Ok(Value::Array(rendered))
}
