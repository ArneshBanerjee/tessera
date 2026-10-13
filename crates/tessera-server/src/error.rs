//! API errors in Jira's format:
//! `{"errorMessages": ["..."], "errors": {"field": "message"}}`.

use std::collections::BTreeMap;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    #[serde(skip)]
    pub status: StatusCode,
    pub error_messages: Vec<String>,
    pub errors: BTreeMap<String, String>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            error_messages: vec![message.into()],
            errors: BTreeMap::new(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "You are not authenticated. Authentication required to perform this operation.",
        )
    }

    /// A validation error tied to one field, as Jira reports them under `errors`.
    pub fn field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error_messages: Vec::new(),
            errors: BTreeMap::from([(field.into(), message.into())]),
        }
    }

    pub fn issue_not_found() -> Self {
        Self::not_found("Issue does not exist or you do not have permission to see it.")
    }

    pub fn project_not_found(id_or_key: &str) -> Self {
        Self::not_found(format!("No project could be found with key '{id_or_key}'."))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(&self)).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Basic realm=\"Tessera\""),
            );
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "database error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error.")
    }
}
