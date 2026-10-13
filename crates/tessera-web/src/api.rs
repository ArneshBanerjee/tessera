//! Calls to the Tessera REST API. The web app uses the same public API as any other client.

use gloo_net::http::{Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The core API, with rich text as ADF.
pub const CORE: &str = "/rest/api/3";
/// The Jira Software API.
pub const AGILE: &str = "/rest/agile/1.0";
/// The Confluence v1 API.
pub const WIKI: &str = "/wiki/rest/api";
/// The Confluence v2 API.
pub const WIKI2: &str = "/wiki/api/v2";

/// An error from the API, with Jira's messages joined for display.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl ApiError {
    /// Reads the messages of a Jira error (`errorMessages`, `errors`), a Confluence v1 error
    /// (`message`), or a Confluence v2 error (`errors[].title`).
    fn from_body(status: u16, body: &Value) -> Self {
        let mut parts: Vec<String> = body["errorMessages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(errors) = body["errors"].as_object() {
            parts.extend(
                errors
                    .values()
                    .filter_map(|v| v.as_str().map(str::to_string)),
            );
        }
        if let Some(errors) = body["errors"].as_array() {
            parts.extend(
                errors
                    .iter()
                    .filter_map(|e| e["title"].as_str().map(str::to_string)),
            );
        }
        if let Some(message) = body["message"].as_str() {
            parts.push(message.to_string());
        }
        let message = if parts.is_empty() {
            format!("The request failed with status {status}.")
        } else {
            parts.join(" ")
        };
        Self { status, message }
    }

    fn network(error: impl std::fmt::Display) -> Self {
        Self {
            status: 0,
            message: format!("Could not reach the server: {error}"),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Sends a request and returns the JSON body (`null` for empty bodies).
pub async fn request(method: Method, path: &str, body: Option<&Value>) -> ApiResult<Value> {
    let builder = RequestBuilder::new(path).method(method);
    let request = match body {
        Some(body) => builder
            .header("Content-Type", "application/json")
            .body(body.to_string()),
        None => builder.build(),
    }
    .map_err(ApiError::network)?;
    let response = request.send().await.map_err(ApiError::network)?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let json = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    if (200..300).contains(&status) {
        Ok(json)
    } else {
        Err(ApiError::from_body(status, &json))
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> ApiResult<T> {
    serde_json::from_value(value).map_err(|e| ApiError {
        status: 0,
        message: format!("Unexpected response: {e}"),
    })
}

pub async fn get<T: DeserializeOwned>(path: &str) -> ApiResult<T> {
    decode(request(Method::GET, path, None).await?)
}

pub async fn post<T: DeserializeOwned>(path: &str, body: &Value) -> ApiResult<T> {
    decode(request(Method::POST, path, Some(body)).await?)
}

pub async fn delete(path: &str) -> ApiResult<()> {
    request(Method::DELETE, path, None).await.map(|_| ())
}

/// Encodes a value for a query string.
pub fn encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
