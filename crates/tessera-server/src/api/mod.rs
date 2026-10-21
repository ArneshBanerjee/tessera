//! The Jira-compatible REST API.
//!
//! Every route is served under both `/rest/api/2` and `/rest/api/3`. The versions differ only in
//! rich text: v2 uses plain strings, v3 uses ADF documents. Handlers read [`ApiVersion`] to pick.

pub mod access;
mod account;
pub mod agile;
pub mod attachments;
mod backup_import;
mod bulk;
mod comments;
pub mod dashboards;
mod everything;
pub mod extract;
mod fields;
pub mod filters;
mod issue_fields;
mod issues;
mod links;
mod meta;
mod notifications;
mod parts;
mod permissions;
mod projects;
pub mod render;
mod schemes;
mod search;
pub mod security;
mod server_info;
mod session;
mod sharing;
mod social;
mod transitions;
mod users;
mod workflows;
mod worklogs;

use axum::extract::DefaultBodyLimit;
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
        .nest("/rest/agile/1.0", agile::router())
        .nest(
            "/rest/tessera/1.0",
            account::router()
                .route(
                    "/dashboard/{id}/favourite",
                    put(dashboards::add_favourite).delete(dashboards::remove_favourite),
                )
                .route("/dashboard/{id}/layout", put(dashboards::set_layout))
                .merge(notifications::router())
                .merge(backup_import::router())
                .route("/search", get(everything::search))
                .merge(agile::reports::tessera_router())
                .merge(agile::quick_filters::tessera_router()),
        )
        .route(
            "/rest/internal/latest/dashboards/{id}/automatic-refresh-ms",
            put(dashboards::set_refresh),
        )
        .nest(
            "/rest/greenhopper/1.0",
            agile::reports::greenhopper_router(),
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
        .route(
            "/statuses",
            post(workflows::create_statuses)
                .put(workflows::update_statuses)
                .delete(workflows::delete_statuses),
        )
        .route("/statuses/search", get(workflows::search_statuses))
        .route("/workflow", post(workflows::create_workflow))
        .route("/workflow/search", get(workflows::search_workflows))
        .route(
            "/workflow/{id}",
            axum::routing::delete(workflows::delete_workflow),
        )
        .route(
            "/workflowscheme",
            get(workflows::schemes).post(workflows::create_scheme),
        )
        .route(
            "/workflowscheme/project",
            get(workflows::project_schemes).put(workflows::assign_scheme),
        )
        .route(
            "/workflowscheme/{id}",
            get(workflows::scheme)
                .put(workflows::update_scheme)
                .delete(workflows::delete_scheme),
        )
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
        .route(
            "/permissionscheme",
            get(permissions::schemes).post(permissions::create_scheme),
        )
        .route(
            "/permissionscheme/{id}",
            get(permissions::scheme)
                .put(permissions::update_scheme)
                .delete(permissions::delete_scheme),
        )
        .route(
            "/permissionscheme/{id}/permission",
            get(permissions::grants).post(permissions::create_grant),
        )
        .route(
            "/permissionscheme/{id}/permission/{grant}",
            get(permissions::get_grant).delete(permissions::delete_grant),
        )
        .route("/permissions", get(permissions::all))
        .route("/mypermissions", get(permissions::mine))
        // Roles and groups
        .route(
            "/role",
            get(permissions::roles).post(permissions::create_role),
        )
        .route(
            "/role/{id}",
            get(permissions::get_role)
                .put(permissions::update_role)
                .post(permissions::update_role)
                .delete(permissions::delete_role),
        )
        .route(
            "/group",
            get(permissions::get_group)
                .post(permissions::create_group)
                .delete(permissions::delete_group),
        )
        .route("/group/member", get(permissions::members))
        .route(
            "/group/user",
            post(permissions::add_member).delete(permissions::remove_member),
        )
        .route("/group/bulk", get(permissions::bulk))
        .route("/groups/picker", get(permissions::picker))
        .route("/user/groups", get(permissions::user_groups))
        .route("/project/{id_or_key}/role", get(permissions::project_roles))
        .route(
            "/project/{id_or_key}/roledetails",
            get(permissions::project_role_details),
        )
        .route(
            "/project/{id_or_key}/role/{role}",
            get(permissions::project_role)
                .post(permissions::add_role_actors)
                .put(permissions::set_role_actors)
                .delete(permissions::remove_role_actor),
        )
        .route(
            "/project/{id_or_key}/permissionscheme",
            get(permissions::project_scheme).put(permissions::assign_project_scheme),
        )
        .route(
            "/issuesecurityschemes",
            get(security::list).post(security::create),
        )
        .route("/issuesecurityschemes/level", get(security::levels))
        .route(
            "/issuesecurityschemes/level/default",
            put(security::set_defaults),
        )
        .route("/issuesecurityschemes/level/member", get(security::members))
        .route(
            "/issuesecurityschemes/project",
            get(security::project_associations).put(security::associate),
        )
        .route(
            "/issuesecurityschemes/{id}",
            get(security::get)
                .put(security::update)
                .delete(security::delete),
        )
        .route(
            "/issuesecurityschemes/{id}/level",
            put(security::add_levels_to_scheme),
        )
        .route(
            "/issuesecurityschemes/{id}/level/{level}",
            put(security::update_level).delete(security::delete_level),
        )
        .route(
            "/issuesecurityschemes/{id}/level/{level}/member",
            put(security::add_members),
        )
        .route(
            "/issuesecurityschemes/{id}/level/{level}/member/{member}",
            axum::routing::delete(security::delete_member),
        )
        .route("/securitylevel/{id}", get(security::level))
        .route(
            "/project/{key}/issuesecuritylevelscheme",
            get(security::project_scheme),
        )
        .route(
            "/project/{key}/securitylevel",
            get(security::project_levels),
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
        .route(
            "/issue/{id_or_key}/watchers",
            get(social::watchers)
                .post(social::add_watcher)
                .delete(social::remove_watcher),
        )
        .route(
            "/issue/{id_or_key}/votes",
            get(social::votes)
                .post(social::add_vote)
                .delete(social::remove_vote),
        )
        .route(
            "/issue/{id_or_key}/worklog",
            get(worklogs::list).post(worklogs::create),
        )
        .route(
            "/issue/{id_or_key}/worklog/{id}",
            get(worklogs::get)
                .put(worklogs::update)
                .delete(worklogs::delete),
        )
        .route(
            "/issue/{id_or_key}/attachments",
            post(attachments::upload).layer(DefaultBodyLimit::max(attachments::UPLOAD_LIMIT)),
        )
        // Attachments
        .route("/attachment/meta", get(attachments::meta))
        .route(
            "/attachment/{id}",
            get(attachments::get).delete(attachments::delete),
        )
        .route("/attachment/content/{id}", get(attachments::content))
        // Issue links
        .route("/issueLink", post(links::create))
        .route("/issueLink/{id}", get(links::get).delete(links::delete))
        .route("/issueLinkType", get(links::types).post(links::create_type))
        .route(
            "/issueLinkType/{id}",
            get(links::get_type)
                .put(links::update_type)
                .delete(links::delete_type),
        )
        // Components and versions
        .route("/component", post(parts::create_component))
        .route(
            "/component/{id}",
            get(parts::get_component)
                .put(parts::update_component)
                .delete(parts::delete_component),
        )
        .route(
            "/component/{id}/relatedIssueCounts",
            get(parts::component_issue_count),
        )
        .route(
            "/project/{id_or_key}/components",
            get(parts::project_components),
        )
        .route(
            "/project/{id_or_key}/component",
            get(parts::project_components_paged),
        )
        .route("/version", post(parts::create_version))
        .route(
            "/version/{id}",
            get(parts::get_version)
                .put(parts::update_version)
                .delete(parts::delete_version),
        )
        .route("/version/{id}/move", post(parts::move_version))
        .route(
            "/version/{id}/relatedIssueCounts",
            get(parts::version_issue_counts),
        )
        .route(
            "/version/{id}/unresolvedIssueCount",
            get(parts::version_unresolved_count),
        )
        .route(
            "/project/{id_or_key}/versions",
            get(parts::project_versions),
        )
        .route(
            "/project/{id_or_key}/version",
            get(parts::project_versions_paged),
        )
        // Bulk operations
        .route("/issue/bulk", post(bulk::create))
        .route(
            "/bulk/issues/fields",
            get(bulk::editable_fields).post(bulk::edit),
        )
        .route(
            "/bulk/issues/transition",
            get(bulk::available_transitions).post(bulk::transition),
        )
        .route("/bulk/issues/delete", post(bulk::delete))
        .route("/bulk/issues/move", post(bulk::move_issues))
        .route("/bulk/issues/watch", post(bulk::watch))
        .route("/bulk/issues/unwatch", post(bulk::unwatch))
        .route("/bulk/queue/{task}", get(bulk::progress))
        // Dashboards
        .route("/dashboard", get(dashboards::list).post(dashboards::create))
        .route("/dashboard/search", get(dashboards::search))
        .route("/dashboard/gadgets", get(dashboards::available))
        .route(
            "/dashboard/{id}",
            get(dashboards::get)
                .put(dashboards::update)
                .delete(dashboards::delete),
        )
        .route("/dashboard/{id}/copy", post(dashboards::copy))
        .route(
            "/dashboard/{id}/gadget",
            get(dashboards::gadgets).post(dashboards::add_gadget),
        )
        .route(
            "/dashboard/{id}/gadget/{gadget}",
            put(dashboards::update_gadget).delete(dashboards::delete_gadget),
        )
        .route(
            "/dashboard/{id}/items/{item}/properties",
            get(dashboards::property_keys),
        )
        .route(
            "/dashboard/{id}/items/{item}/properties/{key}",
            get(dashboards::property)
                .put(dashboards::set_property)
                .delete(dashboards::delete_property),
        )
        // Saved filters
        .route("/filter", post(filters::create))
        .route("/filter/search", get(filters::search))
        .route("/filter/my", get(filters::my))
        .route("/filter/favourite", get(filters::favourites))
        .route(
            "/filter/{id}",
            get(filters::get)
                .put(filters::update)
                .delete(filters::delete),
        )
        .route(
            "/filter/{id}/favourite",
            put(filters::add_favourite).delete(filters::remove_favourite),
        )
        .route("/filter/{id}/permission", get(filters::permissions))
        // Search
        .route("/search", get(search::get).post(search::post))
        .route("/search/jql", get(search::jql_get).post(search::jql_post))
}
