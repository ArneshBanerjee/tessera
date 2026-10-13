//! Errors in the Confluence REST API's format, which differs from Jira's:
//! `{"statusCode": 404, "data": {...}, "message": "...", "reason": "Not Found"}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::ApiError;

pub type WikiResult<T> = Result<T, WikiError>;

#[derive(Debug)]
pub struct WikiError {
    pub status: StatusCode,
    pub message: String,
}

impl WikiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn content_not_found(id: &str) -> Self {
        Self::not_found(format!("No content found with id : ContentId{{id={id}}}"))
    }

    pub fn space_not_found(key: &str) -> Self {
        Self::not_found(format!("No space found with key : {key}"))
    }
}

impl IntoResponse for WikiError {
    fn into_response(self) -> Response {
        let body = json!({
            "statusCode": self.status.as_u16(),
            "data": {
                "authorized": self.status != StatusCode::UNAUTHORIZED && self.status != StatusCode::FORBIDDEN,
                "valid": self.status != StatusCode::BAD_REQUEST,
                "errors": [],
                "successful": false,
            },
            "message": self.message,
            "reason": self.status.canonical_reason().unwrap_or_default(),
        });
        (self.status, axum::Json(body)).into_response()
    }
}

impl From<sqlx::Error> for WikiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "database error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    }
}

impl From<ApiError> for WikiError {
    fn from(error: ApiError) -> Self {
        let message = error
            .error_messages
            .first()
            .cloned()
            .or_else(|| error.errors.values().next().cloned())
            .unwrap_or_default();
        Self::new(error.status, message)
    }
}
