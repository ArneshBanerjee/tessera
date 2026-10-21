//! Restoring Atlassian backups from the web app: `POST /rest/tessera/1.0/admin/import/jira` and
//! `/confluence`, with the backup as a multipart upload. Site administrators only. Very large
//! backups are better imported with `tessera import-jira` or `tessera import-confluence`.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::routing::post;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::extract::Json;
use crate::AppState;
use crate::auth::AdminUser;
use crate::error::{ApiError, ApiResult};
use crate::import;

/// The largest backup the web app takes.
const LIMIT: usize = 4 * 1024 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/import/{product}", post(restore))
        .layer(DefaultBodyLimit::max(LIMIT))
}

async fn restore(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(product): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    if !matches!(product.as_str(), "jira" | "confluence") {
        return Err(ApiError::not_found("Import a jira or confluence backup."));
    }
    // The backup goes to a file first, as it can be far larger than memory.
    let path =
        std::env::temp_dir().join(format!("tessera-import-{}", uuid::Uuid::new_v4().simple()));
    let mut received = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        received = true;
    }
    if !received {
        return Err(ApiError::bad_request("Send the backup as the file part."));
    }
    let result = match product.as_str() {
        "jira" => import::jira::import(&state.db, &state.data_dir, &path).await,
        _ => import::confluence::import(&state.db, &state.data_dir, &path).await,
    };
    let _ = tokio::fs::remove_file(&path).await;
    let summary = result.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let counts: Vec<Value> = summary
        .counts
        .iter()
        .map(|(what, n)| json!({ "what": what, "count": n }))
        .collect();
    Ok(Json(json!({ "counts": counts, "notes": summary.notes })))
}
