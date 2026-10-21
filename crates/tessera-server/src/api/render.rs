//! JSON shapes of the Jira REST API, plus the avatar and icon images they link to.

use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::http::header;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::store::attachments::Attachment;
use crate::store::links::LinkType;
use crate::store::meta::{Catalog, IssueType, Priority, Resolution, Status, StatusCategory};
use crate::store::projects::Project;
use crate::store::users::{self, User};
use crate::store::versions::Version;
use crate::store::worklogs::Worklog;

/// Per-request rendering context: where links point, which API version, and cached lookups.
pub struct Ctx {
    pub state: AppState,
    pub version: ApiVersion,
    pub catalog: Catalog,
    /// The signed-in user, for per-viewer fields such as `isWatching`.
    pub viewer: Option<i64>,
    users: HashMap<i64, Option<User>>,
}

impl Ctx {
    pub async fn new(state: &AppState, version: ApiVersion) -> ApiResult<Self> {
        Ok(Self {
            catalog: Catalog::load(&state.db).await?,
            state: state.clone(),
            version,
            viewer: None,
            users: HashMap::new(),
        })
    }

    /// Sets the user the response is for.
    pub fn viewed_by(mut self, user_id: i64) -> Self {
        self.viewer = Some(user_id);
        self
    }

    /// An absolute URL to a REST resource, for `self` links.
    pub fn api_url(&self, path: &str) -> String {
        format!("{}{}{}", self.state.base_url, self.version.path(), path)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.state.base_url, path)
    }

    /// Loads a user once per request.
    pub async fn load_user(&mut self, id: i64) -> ApiResult<Option<User>> {
        if !self.users.contains_key(&id) {
            let user = users::by_id(&self.state.db, id).await?;
            self.users.insert(id, user);
        }
        Ok(self.users[&id].clone())
    }

    /// A user as JSON, or `null` when `id` is `None` or unknown.
    pub async fn user_ref(&mut self, id: Option<i64>) -> ApiResult<Value> {
        let Some(id) = id else {
            return Ok(Value::Null);
        };
        Ok(match self.load_user(id).await? {
            Some(user) => self.user(&user),
            None => Value::Null,
        })
    }

    pub fn user(&self, user: &User) -> Value {
        json!({
            "self": self.api_url(&format!("/user?accountId={}", user.account_id)),
            "accountId": user.account_id,
            "accountType": "atlassian",
            "emailAddress": user.email,
            "avatarUrls": self.avatar_urls("user", &user.account_id),
            "displayName": user.display_name,
            "active": user.active,
            "timeZone": user.time_zone,
        })
    }

    fn avatar_urls(&self, kind: &str, id: &str) -> Value {
        let url = |size: u32| self.url(&format!("/avatar/{kind}/{id}?size={size}"));
        json!({ "48x48": url(48), "24x24": url(24), "16x16": url(16), "32x32": url(32) })
    }

    pub fn issue_type(&self, issue_type: &IssueType) -> Value {
        json!({
            "self": self.api_url(&format!("/issuetype/{}", issue_type.id)),
            "id": issue_type.id.to_string(),
            "description": issue_type.description,
            "iconUrl": self.url(&format!("/icons/issuetype-{}.svg", icon_slug(&issue_type.name))),
            "name": issue_type.name,
            "untranslatedName": issue_type.name,
            "subtask": issue_type.is_subtask(),
            "hierarchyLevel": issue_type.hierarchy_level,
        })
    }

    pub fn status_category(&self, category: &StatusCategory) -> Value {
        json!({
            "self": self.api_url(&format!("/statuscategory/{}", category.id)),
            "id": category.id,
            "key": category.key,
            "colorName": category.color_name,
            "name": category.name,
        })
    }

    pub fn status(&self, status: &Status) -> Value {
        let category = self
            .catalog
            .status_category(status.category_id)
            .map(|c| self.status_category(c));
        json!({
            "self": self.api_url(&format!("/status/{}", status.id)),
            "description": status.description,
            "iconUrl": self.url("/icons/status.svg"),
            "name": status.name,
            "untranslatedName": status.name,
            "id": status.id.to_string(),
            "statusCategory": category,
        })
    }

    pub fn priority(&self, priority: &Priority) -> Value {
        json!({
            "self": self.api_url(&format!("/priority/{}", priority.id)),
            "iconUrl": self.url(&format!("/icons/priority-{}.svg", icon_slug(&priority.name))),
            "name": priority.name,
            "id": priority.id.to_string(),
            "statusColor": priority.status_color,
            "description": priority.description,
        })
    }

    pub fn resolution(&self, resolution: &Resolution) -> Value {
        json!({
            "self": self.api_url(&format!("/resolution/{}", resolution.id)),
            "id": resolution.id.to_string(),
            "description": resolution.description,
            "name": resolution.name,
        })
    }

    /// A project as it appears inside other objects, such as an issue's `project` field.
    pub fn project_ref(&self, project: &Project) -> Value {
        json!({
            "self": self.api_url(&format!("/project/{}", project.id)),
            "id": project.id.to_string(),
            "key": project.key,
            "name": project.name,
            "projectTypeKey": project.project_type_key,
            "simplified": false,
            "avatarUrls": self.avatar_urls("project", &project.key),
        })
    }

    pub fn version_json(&self, version: &Version) -> Value {
        let mut value = json!({
            "self": self.api_url(&format!("/version/{}", version.id)),
            "id": version.id.to_string(),
            "description": version.description,
            "name": version.name,
            "archived": version.archived,
            "released": version.released,
            "projectId": version.project_id,
        });
        if let Some(start) = &version.start_date {
            value["startDate"] = json!(start);
            value["userStartDate"] = json!(user_date(start));
        }
        if let Some(release) = &version.release_date {
            value["releaseDate"] = json!(release);
            value["userReleaseDate"] = json!(user_date(release));
            // Overdue: not released, and the release date has passed.
            let today = crate::time::format(crate::time::now_ms())[..10].to_string();
            value["overdue"] = json!(!version.released && release.as_str() < today.as_str());
        }
        value
    }

    pub fn link_type_json(&self, link_type: &LinkType) -> Value {
        json!({
            "id": link_type.id.to_string(),
            "name": link_type.name,
            "inward": link_type.inward,
            "outward": link_type.outward,
            "self": self.api_url(&format!("/issueLinkType/{}", link_type.id)),
        })
    }

    pub async fn attachment_json(&mut self, attachment: &Attachment) -> ApiResult<Value> {
        Ok(json!({
            "self": self.api_url(&format!("/attachment/{}", attachment.id)),
            "id": attachment.id.to_string(),
            "filename": attachment.filename,
            "author": self.user_ref(Some(attachment.author_id)).await?,
            "created": crate::time::format(attachment.created_at),
            "size": attachment.size,
            "mimeType": attachment.mime_type,
            "content": self.api_url(&format!("/attachment/content/{}", attachment.id)),
        }))
    }

    pub async fn worklog_json(&mut self, worklog: &Worklog) -> ApiResult<Value> {
        Ok(json!({
            "self": self.api_url(&format!("/issue/{}/worklog/{}", worklog.issue_id, worklog.id)),
            "author": self.user_ref(Some(worklog.author_id)).await?,
            "updateAuthor": self.user_ref(Some(worklog.update_author_id)).await?,
            "comment": self.rich_text(worklog.comment.as_deref()),
            "created": crate::time::format(worklog.created_at),
            "updated": crate::time::format(worklog.updated_at),
            "started": crate::time::format(worklog.started),
            "timeSpent": crate::time::format_duration(worklog.time_spent_seconds),
            "timeSpentSeconds": worklog.time_spent_seconds,
            "id": worklog.id.to_string(),
            "issueId": worklog.issue_id.to_string(),
        }))
    }

    /// Rich text for output: an ADF document in v3, a plain string in v2.
    pub fn rich_text(&self, adf_json: Option<&str>) -> Value {
        let Some(doc) = adf_json.and_then(|json| serde_json::from_str::<Value>(json).ok()) else {
            return Value::Null;
        };
        match self.version {
            ApiVersion::V3 => doc,
            ApiVersion::V2 => serde_json::from_value(doc)
                .map(|node| Value::String(tessera_adf::to_plain_text(&node)))
                .unwrap_or(Value::Null),
        }
    }
}

/// Rich text from a request, stored as ADF JSON. Accepts an ADF document or a plain string in
/// both API versions, so clients written for either version work. `null` clears the field.
pub fn rich_text_input(field: &str, value: &Value) -> ApiResult<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.is_empty() => Ok(None),
        Value::String(text) => Ok(Some(
            serde_json::to_string(&tessera_adf::from_plain_text(text)).expect("ADF serializes"),
        )),
        Value::Object(_) => {
            let node: tessera_adf::Node = serde_json::from_value(value.clone())
                .map_err(|_| ApiError::field(field, "Operation value must be an Atlassian Document (see the Atlassian Document Format)."))?;
            if node.kind != "doc" {
                return Err(ApiError::field(
                    field,
                    "Operation value must be an Atlassian Document (see the Atlassian Document Format).",
                ));
            }
            Ok(Some(serde_json::to_string(&node).expect("ADF serializes")))
        }
        _ => Err(ApiError::field(
            field,
            "Operation value must be a string or an Atlassian Document.",
        )),
    }
}

/// A date as Jira shows it to people: `2026-10-13` becomes `13/Oct/26`.
fn user_date(date: &str) -> String {
    crate::time::parse_date(date)
        .map(|d| d.strftime("%d/%b/%y").to_string())
        .unwrap_or_else(|| date.to_string())
}

/// A file-name-safe version of a name, for icon URLs: "Won't Do" becomes "wont-do".
pub fn icon_slug(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter_map(|c| match c {
            'a'..='z' | '0'..='9' => Some(c),
            ' ' | '-' => Some('-'),
            _ => None,
        })
        .collect()
}

#[derive(Deserialize)]
pub struct AvatarQuery {
    size: Option<u32>,
}

/// A generated avatar: initials on a color picked from the id.
pub async fn avatar(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<AvatarQuery>,
) -> impl IntoResponse {
    let label = match kind.as_str() {
        "user" => users::by_account_id(&state.db, &id)
            .await
            .ok()
            .flatten()
            .map(|user| initials(&user.display_name))
            .unwrap_or_else(|| "?".into()),
        _ => id.chars().take(2).collect::<String>().to_uppercase(),
    };
    let size = query.size.unwrap_or(48).clamp(16, 256);
    let hue = id
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b.into()))
        % 360;
    let radius = if kind == "user" { size / 2 } else { size / 6 };
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}"><rect width="{size}" height="{size}" rx="{radius}" fill="hsl({hue},55%,45%)"/><text x="50%" y="50%" dy=".35em" text-anchor="middle" font-family="system-ui,sans-serif" font-size="{font}" font-weight="600" fill="#fff">{label}</text></svg>"##,
        font = size * 2 / 5,
        label = escape_xml(&label),
    );
    svg_response(svg)
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Icons for issue types, priorities, and statuses. Original artwork, not Atlassian's.
pub async fn icon(Path(name): Path<String>) -> ApiResult<impl IntoResponse> {
    let name = name.strip_suffix(".svg").unwrap_or(&name);
    let square = |color: &str, mark: &str| {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect width="16" height="16" rx="3" fill="{color}"/>{mark}</svg>"##
        )
    };
    let arrow = |color: &str, up: bool, double: bool| {
        let path = |y: i32| {
            if up {
                format!("M4 {} l4 -4 l4 4", y + 4)
            } else {
                format!("M4 {} l4 4 l4 -4", y - 4)
            }
        };
        let mut paths = path(if up { 6 } else { 10 });
        if double {
            paths = format!(
                "{} {}",
                path(if up { 2 } else { 14 }),
                path(if up { 6 } else { 10 })
            );
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="{paths}" fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##
        )
    };
    let svg = match name {
        "issuetype-epic" => square("#904ee2", r##"<path d="M9 3 5 9h3l-1 4 4-6H8z" fill="#fff"/>"##),
        "issuetype-story" => square("#63ba3c", r##"<path d="M5 3h6v10l-3-2-3 2z" fill="#fff"/>"##),
        "issuetype-task" => square("#4bade8", r##"<path d="m4.5 8 2.5 2.5 4.5-5" fill="none" stroke="#fff" stroke-width="2"/>"##),
        "issuetype-bug" => square("#e5493a", r##"<circle cx="8" cy="8" r="3.5" fill="#fff"/>"##),
        "issuetype-subtask" => square("#4bade8", r##"<rect x="4" y="4" width="5" height="5" fill="none" stroke="#fff" stroke-width="1.5"/><rect x="7" y="7" width="5" height="5" fill="#fff"/>"##),
        "priority-highest" => arrow("#d04437", true, true),
        "priority-high" => arrow("#f15c75", true, false),
        "priority-medium" => {
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="M3 6h10M3 10h10" stroke="#f79232" stroke-width="2" stroke-linecap="round"/></svg>"##.to_string()
        }
        "priority-low" => arrow("#2a8735", false, false),
        "priority-lowest" => arrow("#2a8735", false, true),
        "status" => square("#dfe1e6", ""),
        // Custom issue types and priorities share a neutral icon.
        _ if name.starts_with("issuetype-") => square("#6b778c", r##"<circle cx="8" cy="8" r="2" fill="#fff"/>"##),
        _ if name.starts_with("priority-") => arrow("#6b778c", true, false),
        _ => return Err(ApiError::not_found("No such icon.")),
    };
    Ok(svg_response(svg))
}

fn svg_response(svg: String) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        svg,
    )
}
