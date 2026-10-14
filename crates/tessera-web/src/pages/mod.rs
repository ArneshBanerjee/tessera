//! Pages of the app, and helpers they share.

pub mod admin;
pub mod admin_workflows;
pub mod board;
pub mod create;
pub mod filters;
pub mod issue;
pub mod project_settings;
pub mod projects;
pub mod reports;
pub mod search;
pub mod settings;
pub mod wiki;
pub mod your_work;

use leptos::prelude::*;
use serde_json::json;

use crate::api::{self, ApiResult};
use crate::model::{FieldInfo, SearchPage};

/// The fields lists show for each issue.
pub const LIST_FIELDS: &[&str] = &[
    "summary",
    "status",
    "issuetype",
    "priority",
    "assignee",
    "updated",
    "created",
    "project",
];

/// Runs JQL and returns one page of issues with the list fields.
pub async fn search(jql: &str, start_at: i64, max_results: i64) -> ApiResult<SearchPage> {
    api::post(
        &format!("{}/search", api::CORE),
        &json!({ "jql": jql, "startAt": start_at, "maxResults": max_results, "fields": LIST_FIELDS }),
    )
    .await
}

/// Ids of Jira Software's fields, which differ between sites: (Sprint, Story point estimate).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgileFields {
    pub sprint: Option<String>,
    pub story_points: Option<String>,
}

pub fn agile_fields() -> LocalResource<AgileFields> {
    LocalResource::new(|| async {
        let fields: Vec<FieldInfo> = api::get(&format!("{}/field", api::CORE))
            .await
            .unwrap_or_default();
        let id = |name: &str| {
            fields
                .iter()
                .find(|f| f.custom && f.name == name)
                .map(|f| f.id.clone())
        };
        AgileFields {
            sprint: id("Sprint"),
            story_points: id("Story point estimate"),
        }
    })
}
