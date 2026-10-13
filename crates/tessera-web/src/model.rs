//! The parts of the REST API's JSON that the web app reads.
//!
//! Every field has a default, so a response with fewer fields (for example a search that
//! asked only for `summary`) still decodes.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct User {
    pub account_id: String,
    pub display_name: String,
    pub email_address: String,
    pub avatar_urls: HashMap<String, String>,
}

impl User {
    pub fn avatar(&self) -> String {
        self.avatar_urls.get("48x48").cloned().unwrap_or_default()
    }
}

/// A named thing with an id: issue type, priority, resolution, component, version.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Named {
    pub id: String,
    pub name: String,
    pub icon_url: String,
    pub description: String,
    pub subtask: bool,
    pub released: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusCategory {
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    pub id: String,
    pub name: String,
    pub status_category: StatusCategory,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub id: String,
    pub key: String,
    pub name: String,
    pub project_type_key: String,
    pub description: String,
    pub lead: Option<User>,
    pub avatar_urls: HashMap<String, String>,
    pub issue_types: Vec<Named>,
}

impl Project {
    pub fn avatar(&self) -> String {
        self.avatar_urls.get("48x48").cloned().unwrap_or_default()
    }
}

/// A short issue reference, as used for parents, subtasks, and links.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct IssueRef {
    pub id: String,
    pub key: String,
    pub fields: RefFields,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RefFields {
    pub summary: String,
    pub status: Option<Status>,
    pub issuetype: Option<Named>,
    pub priority: Option<Named>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Comment {
    pub id: String,
    pub author: Option<User>,
    pub body: Value,
    pub created: String,
    pub updated: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentPage {
    pub comments: Vec<Comment>,
    pub total: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LinkType {
    pub name: String,
    pub inward: String,
    pub outward: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct IssueLink {
    pub id: String,
    #[serde(rename = "type")]
    pub link_type: LinkType,
    pub inward_issue: Option<IssueRef>,
    pub outward_issue: Option<IssueRef>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub size: i64,
    pub mime_type: String,
    pub content: String,
    pub created: String,
    pub author: Option<User>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Fields {
    pub summary: String,
    pub description: Value,
    pub issuetype: Option<Named>,
    pub status: Option<Status>,
    pub priority: Option<Named>,
    pub resolution: Option<Named>,
    pub assignee: Option<User>,
    pub reporter: Option<User>,
    pub project: Option<Project>,
    pub labels: Vec<String>,
    pub created: String,
    pub updated: String,
    pub duedate: Option<String>,
    pub parent: Option<IssueRef>,
    pub subtasks: Vec<IssueRef>,
    pub comment: Option<CommentPage>,
    pub components: Vec<Named>,
    pub fix_versions: Vec<Named>,
    pub issuelinks: Vec<IssueLink>,
    pub attachment: Vec<Attachment>,
    /// Custom fields and anything else, by field id.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Issue {
    pub id: String,
    pub key: String,
    pub fields: Fields,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Transition {
    pub id: String,
    pub name: String,
    pub to: Status,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchPage {
    pub issues: Vec<Issue>,
    pub total: i64,
    pub start_at: i64,
}

/// A page of values, as the Agile and filter APIs return them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Page<T> {
    pub values: Vec<T>,
    pub total: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Board {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub board_type: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct IdRef {
    pub id: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardColumn {
    pub name: String,
    pub statuses: Vec<IdRef>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ColumnConfig {
    pub columns: Vec<BoardColumn>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardConfig {
    pub column_config: ColumnConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Sprint {
    pub id: i64,
    pub name: String,
    pub state: String,
    pub goal: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub id: String,
    pub name: String,
    pub jql: String,
    pub owner: Option<User>,
    pub favourite: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FieldInfo {
    pub id: String,
    pub name: String,
    pub custom: bool,
}

/// A date-time from the API, shortened for display: `2026-10-13T14:43:15.245+0000` becomes
/// `2026-10-13 14:43`.
pub fn short_time(value: &str) -> String {
    if value.len() >= 16 {
        format!("{} {}", &value[..10], &value[11..16])
    } else {
        value.to_string()
    }
}
