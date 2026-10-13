//! A JSON extractor that reports bad request bodies the way Jira does: HTTP 400 with
//! `errorMessages`, instead of axum's plain-text 422.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// Use in place of `axum::Json` for request bodies and responses.
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Json(value)),
            Err(rejection) => Err(rejection_error(rejection)),
        }
    }
}

fn rejection_error(rejection: JsonRejection) -> ApiError {
    let detail = match &rejection {
        JsonRejection::MissingJsonContentType(_) => {
            "The request must have the header Content-Type: application/json.".to_string()
        }
        other => format!("The request body is not valid: {}", other.body_text()),
    };
    let status = match rejection {
        JsonRejection::MissingJsonContentType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::new(status, detail)
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
