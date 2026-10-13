//! Serves the web app that `trunk build` writes to `crates/tessera-web/dist`.
//!
//! The files are embedded in the binary at compile time. When the web app was not built first,
//! the server still runs and serves only the API.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../tessera-web/dist"]
#[allow_missing = true]
struct Assets;

/// Answers every request that no API route matched.
///
/// A known file is served as is. Any other path gets `index.html`, so the web app can route it.
pub async fn fallback(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Unknown API paths stay a 404 instead of getting the web app.
    if ["rest/", "wiki/rest/", "wiki/api/"]
        .iter()
        .any(|api| path.starts_with(api))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(file) = Assets::get(path).filter(|_| !path.is_empty()) {
        // Trunk puts a content hash in every asset name, so assets never change.
        return (
            [
                (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_string(),
                ),
            ],
            file.data,
        )
            .into_response();
    }
    match Assets::get("index.html") {
        Some(index) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            index.data,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "The web app is not built. Run `trunk build --release` in crates/tessera-web, then rebuild the server.",
        )
            .into_response(),
    }
}
