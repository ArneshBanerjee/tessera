//! The Jira-compatible REST API.
//!
//! Every route is served under both `/rest/api/2` and `/rest/api/3`. The versions differ only in
//! rich text: v2 uses plain strings, v3 uses ADF documents. Handlers read [`ApiVersion`] to pick.

mod comments;
mod fields;
mod issue_fields;
mod issues;
mod meta;
mod projects;
pub mod render;
mod schemes;
mod search;
mod server_info;
mod session;
mod transitions;
mod users;

use axum::routing::{get, post, put};
use axum::{Extension, Router};
use serde::Deserialize;

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVersion {
    V2,
    V3,
}

impl ApiVersion {
    pub fn path(self) -> &'static str {
        match self {
            Self::V2 => "/rest/api/2",
            Self::V3 => "/rest/api/3",
        }
    }
}

/// `startAt` and `maxResults`, Jira's paging parameters.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(default)]
    pub start_at: i64,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

fn default_max_results() -> i64 {
    50
}

impl Page {
    /// Clamps the page to sane bounds, as Jira does.
    pub fn bounded(&self, limit: i64) -> (i64, i64) {
        (self.start_at.max(0), self.max_results.clamp(0, limit))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/avatar/{kind}/{id}", get(render::avatar))
        .route("/icons/{name}", get(render::icon))
        .route(
            "/rest/auth/1/session",
            get(session::current)
                .post(session::login)
                .delete(session::logout),
        )
        .nest(
            "/rest/api/2",
            jira_routes().layer(Extension(ApiVersion::V2)),
        )
        .nest(
            "/rest/api/3",
            jira_routes().layer(Extension(ApiVersion::V3)),
        )
        // `latest` is an alias that Jira clients use; it serves the v2 representation.
        .nest(
            "/rest/api/latest",
            jira_routes().layer(Extension(ApiVersion::V2)),
        )
}

fn jira_routes() -> Router<AppState> {
    Router::new()
        .route("/serverInfo", get(server_info::get))
        // Users
        .route("/myself", get(users::myself))
        .route("/user", get(users::get).post(users::create))
        .route("/user/search", get(users::search))
        .route("/users/search", get(users::list))
        .route("/user/assignable/search", get(users::search))
        // Issue metadata
        .route("/issuetype", get(meta::issue_types))
        .route("/issuetype/{id}", get(meta::issue_type))
        .route("/status", get(meta::statuses))
        .route("/status/{id_or_name}", get(meta::status))
        .route("/statuscategory", get(meta::status_categories))
        .route("/priority", get(meta::priorities))
        .route("/priority/{id}", get(meta::priority))
        .route("/resolution", get(meta::resolutions))
        .route("/resolution/{id}", get(meta::resolution))
        .route("/field", get(fields::list).post(fields::create))
        .route("/field/{field_id}/context", get(fields::contexts))
        .route(
            "/field/{field_id}/context/{context_id}/option",
            get(fields::options).post(fields::add_options),
        )
        // Schemes and categories
        .route("/permissionscheme", get(schemes::permission_schemes))
        .route(
            "/permissionscheme/{id}",
            get(schemes::permission_scheme_by_id),
        )
        .route(
            "/issuesecurityschemes",
            get(schemes::issue_security_schemes),
        )
        .route("/notificationscheme", get(schemes::notification_schemes))
        .route(
            "/projectCategory",
            get(schemes::categories).post(schemes::create_category),
        )
        .route(
            "/projectCategory/{id}",
            get(schemes::category)
                .put(schemes::update_category)
                .delete(schemes::delete_category),
        )
        // Projects
        .route("/project", get(projects::list).post(projects::create))
        .route("/project/search", get(projects::search))
        .route(
            "/project/{id_or_key}",
            get(projects::get)
                .put(projects::update)
                .delete(projects::delete),
        )
        .route("/project/{id_or_key}/statuses", get(projects::statuses))
        // Issues
        .route("/issue", post(issues::create))
        .route("/issue/createmeta", get(issues::create_meta))
        .route(
            "/issue/createmeta/{project}/issuetypes",
            get(issues::create_meta_issue_types),
        )
        .route(
            "/issue/createmeta/{project}/issuetypes/{issue_type}",
            get(issues::create_meta_fields),
        )
        .route(
            "/issue/{id_or_key}",
            get(issues::get).put(issues::edit).delete(issues::delete),
        )
        .route("/issue/{id_or_key}/editmeta", get(issues::edit_meta))
        .route("/issue/{id_or_key}/assignee", put(issues::assign))
        .route("/issue/{id_or_key}/changelog", get(issues::changelog))
        .route(
            "/issue/{id_or_key}/transitions",
            get(transitions::list).post(transitions::perform),
        )
        .route(
            "/issue/{id_or_key}/comment",
            get(comments::list).post(comments::create),
        )
        .route(
            "/issue/{id_or_key}/comment/{id}",
            get(comments::get)
                .put(comments::update)
                .delete(comments::delete),
        )
        // Search
        .route("/search", get(search::get).post(search::post))
        .route("/search/jql", get(search::jql_get).post(search::jql_post))
}
