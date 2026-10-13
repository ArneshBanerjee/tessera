//! Fields: `/field`, plus the context and option endpoints for select-list custom fields.
//!
//! Tessera gives every custom field one global context, whose id equals the field id.

use super::extract::Json;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::fields::{self, CustomField};

/// A Jira system field.
pub struct SystemField {
    pub id: &'static str,
    pub name: &'static str,
    /// The schema type: string, number, date, datetime, user, array, and so on.
    pub schema_type: &'static str,
    /// For arrays, the type of the items.
    pub items: Option<&'static str>,
    /// Names JQL accepts for this field.
    pub clause_names: &'static [&'static str],
    /// Whether clients can set it on create or edit.
    pub settable: bool,
}

const fn field(
    id: &'static str,
    name: &'static str,
    schema_type: &'static str,
    clause_names: &'static [&'static str],
    settable: bool,
) -> SystemField {
    SystemField {
        id,
        name,
        schema_type,
        items: None,
        clause_names,
        settable,
    }
}

/// The system fields Tessera supports, in Jira's naming.
pub const SYSTEM_FIELDS: &[SystemField] = &[
    field("summary", "Summary", "string", &["summary"], true),
    field(
        "description",
        "Description",
        "string",
        &["description"],
        true,
    ),
    field(
        "environment",
        "Environment",
        "string",
        &["environment"],
        true,
    ),
    field(
        "issuetype",
        "Issue Type",
        "issuetype",
        &["issuetype", "type"],
        true,
    ),
    field("project", "Project", "project", &["project"], true),
    field("status", "Status", "status", &["status"], false),
    field(
        "statusCategory",
        "Status Category",
        "statusCategory",
        &["statusCategory"],
        false,
    ),
    field("priority", "Priority", "priority", &["priority"], true),
    field(
        "resolution",
        "Resolution",
        "resolution",
        &["resolution"],
        true,
    ),
    field(
        "resolutiondate",
        "Resolved",
        "datetime",
        &["resolved", "resolutiondate"],
        false,
    ),
    field("assignee", "Assignee", "user", &["assignee"], true),
    field("reporter", "Reporter", "user", &["reporter"], true),
    field("creator", "Creator", "user", &["creator"], false),
    field("parent", "Parent", "issuelink", &["parent"], true),
    field("duedate", "Due date", "date", &["due", "duedate"], true),
    field(
        "created",
        "Created",
        "datetime",
        &["created", "createdDate"],
        false,
    ),
    field(
        "updated",
        "Updated",
        "datetime",
        &["updated", "updatedDate"],
        false,
    ),
    SystemField {
        items: Some("string"),
        ..field("labels", "Labels", "array", &["labels"], true)
    },
    SystemField {
        items: Some("issuelinks"),
        ..field("subtasks", "Sub-tasks", "array", &["subtasks"], false)
    },
    SystemField {
        items: Some("comment"),
        ..field("comment", "Comment", "comments-page", &["comment"], false)
    },
    SystemField {
        items: Some("component"),
        ..field("components", "Components", "array", &["component"], true)
    },
    SystemField {
        items: Some("version"),
        ..field(
            "fixVersions",
            "Fix versions",
            "array",
            &["fixVersion"],
            true,
        )
    },
    SystemField {
        items: Some("version"),
        ..field(
            "versions",
            "Affects versions",
            "array",
            &["affectedVersion"],
            true,
        )
    },
    field("timetracking", "Time tracking", "timetracking", &[], true),
    field("timespent", "Time Spent", "number", &["timespent"], false),
    field(
        "timeoriginalestimate",
        "Original estimate",
        "number",
        &["originalEstimate", "timeoriginalestimate"],
        false,
    ),
    field(
        "timeestimate",
        "Remaining Estimate",
        "number",
        &["remainingEstimate", "timeestimate"],
        false,
    ),
    field("workratio", "Work Ratio", "number", &["workratio"], false),
    SystemField {
        items: Some("attachment"),
        ..field("attachment", "Attachment", "array", &["attachments"], false)
    },
    SystemField {
        items: Some("issuelinks"),
        ..field(
            "issuelinks",
            "Linked Issues",
            "array",
            &["issueLink"],
            false,
        )
    },
    field(
        "watches",
        "Watchers",
        "watches",
        &["watcher", "watchers"],
        false,
    ),
    field("votes", "Votes", "votes", &["voter", "votes"], false),
    field(
        "worklog",
        "Log Work",
        "array",
        &["worklogAuthor", "worklogComment", "worklogDate"],
        false,
    ),
    field(
        "issuekey",
        "Key",
        "string",
        &["id", "issue", "issuekey", "key"],
        false,
    ),
];

/// `customfield_10010` for a custom field id.
pub fn custom_field_key(id: i64) -> String {
    format!("customfield_{id}")
}

pub fn system_field_json(field: &SystemField) -> Value {
    let mut schema = json!({ "type": field.schema_type, "system": field.id });
    if let Some(items) = field.items {
        schema["items"] = json!(items);
    }
    json!({
        "id": field.id,
        "key": field.id,
        "name": field.name,
        "custom": false,
        "orderable": field.settable,
        "navigable": true,
        "searchable": true,
        "clauseNames": field.clause_names,
        "schema": schema,
    })
}

pub fn custom_field_json(field: &CustomField) -> Value {
    let (schema_type, custom_key) = fields::jira_schema(&field.field_type);
    let mut schema = json!({ "type": schema_type, "custom": custom_key, "customId": field.id });
    match field.field_type.as_str() {
        "sprint" => schema["items"] = json!("json"),
        "options" => schema["items"] = json!("option"),
        "labels" => schema["items"] = json!("string"),
        _ => {}
    }
    let key = custom_field_key(field.id);
    json!({
        "id": key,
        "key": key,
        "name": field.name,
        "untranslatedName": field.name,
        "custom": true,
        "orderable": true,
        "navigable": true,
        "searchable": true,
        "clauseNames": [format!("cf[{}]", field.id), field.name],
        "schema": schema,
    })
}

pub async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let mut all: Vec<Value> = SYSTEM_FIELDS.iter().map(system_field_json).collect();
    all.extend(fields::list(&state.db).await?.iter().map(custom_field_json));
    Ok(Json(all))
}

#[derive(Deserialize)]
pub struct NewField {
    name: String,
    #[serde(default)]
    description: String,
    /// A Jira custom field type key, for example
    /// `com.atlassian.jira.plugin.system.customfieldtypes:float`.
    #[serde(rename = "type")]
    field_type: String,
}

/// Creates a custom field. Site administrators only.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<NewField>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if request.name.trim().is_empty() {
        return Err(ApiError::field(
            "name",
            "The custom field name must not be empty.",
        ));
    }
    let field_type = fields::type_from_jira_key(&request.field_type).ok_or_else(|| {
        ApiError::field(
            "type",
            format!(
                "Custom field type '{}' is not supported.",
                request.field_type
            ),
        )
    })?;
    if fields::BUILT_IN_TYPES.contains(&field_type) {
        return Err(ApiError::field(
            "type",
            "Tessera creates this field itself; another one cannot be added.",
        ));
    }
    let field = fields::create(
        &state.db,
        request.name.trim(),
        &request.description,
        field_type,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(custom_field_json(&field))))
}

/// Parses `customfield_10010` into the field and checks that it exists.
async fn custom_field(state: &AppState, field_key: &str) -> ApiResult<CustomField> {
    let id: Option<i64> = field_key
        .strip_prefix("customfield_")
        .and_then(|id| id.parse().ok());
    let all = fields::list(&state.db).await?;
    id.and_then(|id| all.into_iter().find(|f| f.id == id))
        .ok_or_else(|| ApiError::not_found(format!("The custom field was not found: {field_key}")))
}

fn paged(values: Vec<Value>) -> Value {
    let total = values.len();
    json!({ "maxResults": total.max(50), "startAt": 0, "total": total, "isLast": true, "values": values })
}

pub async fn contexts(
    State(state): State<AppState>,
    Extension(_): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(field_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let field = custom_field(&state, &field_key).await?;
    Ok(Json(paged(vec![json!({
        "id": field.id.to_string(),
        "name": format!("Default Configuration Scheme for {}", field.name),
        "description": "",
        "isGlobalContext": true,
        "isAnyIssueType": true,
    })])))
}

/// Checks that `context_id` is the field's single context.
fn check_context(field: &CustomField, context_id: &str) -> ApiResult<()> {
    if context_id == field.id.to_string() {
        Ok(())
    } else {
        Err(ApiError::not_found(format!(
            "The context was not found: {context_id}"
        )))
    }
}

pub async fn options(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((field_key, context_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let field = custom_field(&state, &field_key).await?;
    check_context(&field, &context_id)?;
    let options = fields::options(&state.db, field.id).await?;
    Ok(Json(paged(
        options
            .iter()
            .map(|o| json!({ "id": o.id.to_string(), "value": o.value, "disabled": o.disabled }))
            .collect(),
    )))
}

#[derive(Deserialize)]
pub struct NewOptions {
    options: Vec<NewOption>,
}

#[derive(Deserialize)]
pub struct NewOption {
    value: String,
}

/// Adds options to a select-list custom field. Site administrators only.
pub async fn add_options(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((field_key, context_id)): Path<(String, String)>,
    Json(request): Json<NewOptions>,
) -> ApiResult<Json<Value>> {
    let field = custom_field(&state, &field_key).await?;
    check_context(&field, &context_id)?;
    if !matches!(field.field_type.as_str(), "option" | "options") {
        return Err(ApiError::bad_request(
            "This custom field does not support options.",
        ));
    }
    let mut created = Vec::new();
    for option in request.options {
        let option = fields::add_option(&state.db, field.id, option.value.trim()).await?;
        created
            .push(json!({ "id": option.id.to_string(), "value": option.value, "disabled": false }));
    }
    Ok(Json(json!({ "options": created })))
}
