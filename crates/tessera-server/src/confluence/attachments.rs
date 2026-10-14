//! Attachments: `/content/{id}/child/attachment` in the v1 API, and file downloads at
//! `/wiki/download/attachments/{pageId}/{filename}`.

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::Params;
use super::error::{WikiError, WikiResult};
use super::render::WikiCtx;
use super::service::{self, Upload, find_content, find_current};
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::content::{self, Content};
use crate::store::wiki_files;

/// One file part of a multipart upload, with the comment sent for it.
struct Part {
    filename: String,
    media_type: String,
    bytes: Vec<u8>,
    comment: String,
}

/// Uploads must carry `X-Atlassian-Token: no-check`, which a form on another site cannot set.
fn check_token(headers: &HeaderMap) -> WikiResult<()> {
    let token = headers
        .get("X-Atlassian-Token")
        .and_then(|v| v.to_str().ok());
    if !token.is_some_and(|t| t.eq_ignore_ascii_case("no-check")) {
        return Err(WikiError::new(StatusCode::FORBIDDEN, "XSRF check failed"));
    }
    Ok(())
}

/// Reads the `file` parts of an upload. `comment` parts belong to the files in order, and
/// `minorEdit` applies to all of them.
async fn read_parts(mut multipart: Multipart) -> WikiResult<(Vec<Part>, bool)> {
    let bad = |e: axum::extract::multipart::MultipartError| {
        WikiError::bad_request(format!("Could not read the upload: {e}"))
    };
    let mut parts: Vec<Part> = Vec::new();
    let mut comments: Vec<String> = Vec::new();
    let mut minor_edit = false;
    while let Some(field) = multipart.next_field().await.map_err(bad)? {
        match field.name() {
            Some("file") => {
                let filename = field.file_name().unwrap_or("attachment").to_string();
                let media_type = service::media_type(field.content_type(), &filename);
                let bytes = field.bytes().await.map_err(bad)?.to_vec();
                parts.push(Part {
                    filename,
                    media_type,
                    bytes,
                    comment: String::new(),
                });
            }
            Some("comment") => comments.push(field.text().await.map_err(bad)?),
            Some("minorEdit") => minor_edit = field.text().await.map_err(bad)? == "true",
            _ => {}
        }
    }
    if parts.is_empty() {
        return Err(WikiError::bad_request(
            "The request must contain a file in a part named 'file'",
        ));
    }
    for (part, comment) in parts.iter_mut().zip(comments) {
        part.comment = comment;
    }
    Ok((parts, minor_edit))
}

/// Attaches every part. With `replace`, a file whose name is taken becomes a new version.
async fn attach_parts(
    state: &AppState,
    user: &crate::store::users::User,
    container: &Content,
    parts: Vec<Part>,
    minor_edit: bool,
    replace: bool,
) -> WikiResult<Vec<Content>> {
    let mut saved = Vec::with_capacity(parts.len());
    for part in &parts {
        let existing = if replace {
            content::attachments(&state.db, container.id)
                .await?
                .into_iter()
                .find(|a| a.title == part.filename)
        } else {
            None
        };
        let upload = Upload {
            filename: &part.filename,
            media_type: &part.media_type,
            comment: &part.comment,
            minor_edit,
            bytes: &part.bytes,
        };
        saved.push(service::attach(state, user, container, existing.as_ref(), upload).await?);
    }
    Ok(saved)
}

async fn render_list(
    state: &AppState,
    items: &[Content],
    params: &Params,
    path: &str,
) -> WikiResult<Value> {
    let (start, limit) = params.page(50, 500);
    let more = items.len() as i64 > start + limit;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(state);
    let mut results = Vec::new();
    for item in items.iter().skip(start as usize).take(limit as usize) {
        results.push(ctx.content(item, &expand).await?);
    }
    Ok(ctx.collection(results, start, limit, path, more))
}

/// `GET /content/{id}/child/attachment`, optionally filtered by `filename` and `mediaType`.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    params: Params,
) -> WikiResult<Json<Value>> {
    let container = find_current(&state, &user, &id).await?;
    let mut items = content::attachments(&state.db, container.id).await?;
    if let Some(filename) = params.get("filename") {
        items.retain(|a| a.title == filename);
    }
    if let Some(media_type) = params.get("mediaType") {
        let mut kept = Vec::new();
        for item in items {
            if wiki_files::find(&state.db, item.id, None)
                .await?
                .is_some_and(|f| f.media_type == media_type)
            {
                kept.push(item);
            }
        }
        items = kept;
    }
    let path = format!("/content/{id}/child/attachment");
    Ok(Json(render_list(&state, &items, &params, &path).await?))
}

/// `POST /content/{id}/child/attachment`: new attachments. A taken name is an error.
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    params: Params,
    multipart: Multipart,
) -> WikiResult<Json<Value>> {
    check_token(&headers)?;
    let container = find_current(&state, &user, &id).await?;
    let (parts, minor_edit) = read_parts(multipart).await?;
    let saved = attach_parts(&state, &user, &container, parts, minor_edit, false).await?;
    let path = format!("/content/{id}/child/attachment");
    Ok(Json(render_list(&state, &saved, &params, &path).await?))
}

/// `PUT /content/{id}/child/attachment`: new attachments, or new versions of existing ones.
pub async fn create_or_update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    params: Params,
    multipart: Multipart,
) -> WikiResult<Json<Value>> {
    check_token(&headers)?;
    let container = find_current(&state, &user, &id).await?;
    let (parts, minor_edit) = read_parts(multipart).await?;
    let saved = attach_parts(&state, &user, &container, parts, minor_edit, true).await?;
    let path = format!("/content/{id}/child/attachment");
    Ok(Json(render_list(&state, &saved, &params, &path).await?))
}

/// `POST /content/{id}/child/attachment/{attachmentId}/data`: a new version of one attachment.
pub async fn update_data(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, attachment_id)): Path<(String, String)>,
    headers: HeaderMap,
    params: Params,
    multipart: Multipart,
) -> WikiResult<Json<Value>> {
    check_token(&headers)?;
    let container = find_current(&state, &user, &id).await?;
    let attachment = find_current(&state, &user, &attachment_id).await?;
    if attachment.content_type != "attachment" || attachment.container_id != Some(container.id) {
        return Err(WikiError::content_not_found(&attachment_id));
    }
    let (mut parts, minor_edit) = read_parts(multipart).await?;
    let part = parts.remove(0);
    // A new file name renames the attachment, as in Confluence.
    let upload = Upload {
        filename: &part.filename,
        media_type: &part.media_type,
        comment: &part.comment,
        minor_edit,
        bytes: &part.bytes,
    };
    let saved = service::attach(&state, &user, &container, Some(&attachment), upload).await?;
    Ok(Json(
        WikiCtx::new(&state)
            .content(&saved, &params.expand())
            .await?,
    ))
}

/// Media types a browser may show in the page. Anything else downloads, so an uploaded HTML or
/// SVG file can never run script on Tessera's origin.
fn shown_inline(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "application/pdf"
    )
}

/// `GET /wiki/download/attachments/{pageId}/{filename}`, with an optional `version`.
pub async fn download(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((page_id, filename)): Path<(String, String)>,
    params: Params,
) -> WikiResult<Response> {
    let container = find_content(&state, &user, &page_id).await?;
    let attachment = content::attachments(&state.db, container.id)
        .await?
        .into_iter()
        .find(|a| a.title == filename)
        .ok_or_else(|| WikiError::not_found(format!("No attachment named {filename}")))?;
    let file = wiki_files::find(&state.db, attachment.id, params.int("version"))
        .await?
        .ok_or_else(|| WikiError::not_found("No such version of the attachment"))?;
    let bytes = tokio::fs::read(wiki_files::path(
        &state.data_dir,
        attachment.id,
        file.version,
    ))
    .await
    .map_err(|_| WikiError::not_found("The attachment file is missing"))?;
    // Quotes, backslashes, and control characters would break the header.
    let safe_name: String = filename
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let (media_type, disposition) = if shown_inline(&file.media_type) {
        (file.media_type.clone(), "inline")
    } else {
        ("application/octet-stream".to_string(), "attachment")
    };
    Ok((
        [
            (header::CONTENT_TYPE, media_type),
            (
                header::CONTENT_DISPOSITION,
                format!("{disposition}; filename=\"{safe_name}\""),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        Body::from(bytes),
    )
        .into_response())
}
