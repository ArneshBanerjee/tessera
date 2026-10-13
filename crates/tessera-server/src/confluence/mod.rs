//! The Confluence REST API, served under `/wiki` as Confluence Cloud does.
//!
//! - `/wiki/rest/api`: the v1 API (spaces, content, children, history, labels, CQL search).
//! - `/wiki/api/v2`: the v2 API (spaces, pages, blog posts, footer comments, versions, labels).

mod content;
pub mod cql;
pub mod error;
pub mod render;
mod search;
pub mod service;
mod spaces;
mod v2;

use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::routing::{get, post};
use serde_json::Value;

use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::users;
use error::{WikiError, WikiResult};
use render::WikiCtx;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/wiki/rest/api", v1())
        .nest("/wiki/api/v2", v2::router())
}

fn v1() -> Router<AppState> {
    Router::new()
        .route("/space", get(spaces::list).post(spaces::create))
        .route(
            "/space/{key}",
            get(spaces::get).put(spaces::update).delete(spaces::delete),
        )
        .route("/space/{key}/content", get(spaces::content))
        .route("/space/{key}/content/{kind}", get(spaces::content_of_type))
        .route("/content", get(content::list).post(content::create))
        .route("/content/search", get(search::content_search))
        .route(
            "/content/{id}",
            get(content::get)
                .put(content::update)
                .delete(content::delete),
        )
        .route("/content/{id}/child", get(content::children))
        .route("/content/{id}/child/{kind}", get(content::children_of_type))
        .route("/content/{id}/descendant", get(content::descendants))
        .route(
            "/content/{id}/descendant/{kind}",
            get(content::descendants_of_type),
        )
        .route("/content/{id}/history", get(content::history))
        .route(
            "/content/{id}/version",
            get(content::versions).post(content::restore_version),
        )
        .route("/content/{id}/version/{number}", get(content::version))
        .route(
            "/content/{id}/label",
            get(content::labels)
                .post(content::add_labels)
                .delete(content::remove_label_by_query),
        )
        .route(
            "/content/{id}/label/{label}",
            axum::routing::delete(content::remove_label),
        )
        .route(
            "/content/{id}/move/{position}/{target}",
            axum::routing::put(content::move_page),
        )
        .route("/search", get(search::search))
        .route("/user/current", get(current_user))
        .route("/user", get(user))
        .route("/settings/systemInfo", get(system_info))
        .route("/longtask/{id}", get(long_task))
        .route("/contentbody/convert/{to}", post(convert_body))
}

/// Query parameters as a list, since Confluence repeats keys such as `spaceKey`.
pub struct Params(Vec<(String, String)>);

impl Params {
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of a repeated key. Values may also be comma-separated.
    pub fn all(&self, key: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split(','))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    /// `start` and `limit`, bounded as Confluence bounds them.
    pub fn page(&self, default_limit: i64, max_limit: i64) -> (i64, i64) {
        let start = self.int("start").unwrap_or(0).max(0);
        let limit = self
            .int("limit")
            .unwrap_or(default_limit)
            .clamp(0, max_limit);
        (start, limit)
    }

    pub fn expand(&self) -> render::Expand {
        render::Expand::parse(self.get("expand"))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or_default();
        Ok(Params(
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
        ))
    }
}

async fn current_user(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> WikiResult<Json<Value>> {
    Ok(Json(WikiCtx::new(&state).user(&user)))
}

async fn user(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let account_id = params
        .get("accountId")
        .ok_or_else(|| WikiError::bad_request("accountId is required"))?;
    let user = users::by_account_id(&state.db, account_id)
        .await?
        .ok_or_else(|| {
            WikiError::not_found(format!("No user found with accountId : {account_id}"))
        })?;
    Ok(Json(WikiCtx::new(&state).user(&user)))
}

async fn system_info(CurrentUser(_): CurrentUser) -> Json<Value> {
    Json(serde_json::json!({
        "cloudId": "tessera",
        "commitHash": env!("CARGO_PKG_VERSION"),
        "baseUrl": "",
        "edition": "tessera",
        "siteTitle": "Tessera",
        "defaultLocale": "en_US",
        "defaultTimeZone": "UTC",
    }))
}

/// Tessera finishes long tasks, such as deleting a space, before it answers. Any task id reads
/// as complete.
async fn long_task(
    CurrentUser(_): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<Value> {
    Json(serde_json::json!({
        "id": id,
        "name": { "key": "com.atlassian.confluence.longtask" },
        "percentageComplete": 100,
        "successful": true,
        "finished": true,
        "messages": [],
        "status": "COMPLETED",
    }))
}

/// Converts a body between representations. Storage is the only source format.
async fn convert_body(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    axum::extract::Path(to): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> WikiResult<Json<Value>> {
    let value = body["value"].as_str().unwrap_or_default();
    let from = body["representation"].as_str().unwrap_or("storage");
    if from != "storage" && from != "editor" {
        return Err(WikiError::bad_request(format!(
            "Cannot convert from {from} to {to}"
        )));
    }
    let converted = match to.as_str() {
        "storage" | "editor" => value.to_string(),
        "view" | "export_view" | "styled_view" | "anonymous_export_view" => {
            service::view_html(value, &service::load_names(&state.db).await?)
        }
        _ => {
            return Err(WikiError::bad_request(format!(
                "Cannot convert from {from} to {to}"
            )));
        }
    };
    Ok(Json(serde_json::json!({
        "value": converted,
        "representation": to,
        "embeddedContent": [],
    })))
}
