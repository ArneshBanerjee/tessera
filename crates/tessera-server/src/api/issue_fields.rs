//! Turning request JSON into issue field values, and field changes into history entries.
//!
//! Jira clients send `fields` (new values) and `update` (operations such as `add` and `remove`).
//! Both are applied to an [`IssueFields`], which the store then saves.

use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::render::{Ctx, rich_text_input};
use crate::error::{ApiError, ApiResult};
pub use crate::store::fields::CustomCatalog;
use crate::store::fields::CustomField;
use crate::store::issues::{self, ChangeItem, IssueFields};
use crate::store::meta::{find_by_ref, json_id};
use crate::store::projects::Project;
use crate::store::{components, users, versions};
use crate::time;

/// Whether the request creates a new issue or edits an existing one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Create,
    Edit,
}

fn cannot_set(field: &str) -> ApiError {
    ApiError::field(
        field,
        format!("Field '{field}' cannot be set. It is not on the appropriate screen, or unknown."),
    )
}

/// Applies `fields` and `update` from a request. Returns comment bodies from `update.comment`,
/// which the caller adds after saving.
pub async fn apply(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    project: &Project,
    target: &mut IssueFields,
    fields: &Map<String, Value>,
    update: &Map<String, Value>,
    mode: Mode,
) -> ApiResult<Vec<tessera_adf::Node>> {
    for (name, value) in fields {
        set_field(ctx, custom, project, target, name, value, mode).await?;
    }
    let mut comments = Vec::new();
    for (name, operations) in update {
        let operations = operations
            .as_array()
            .ok_or_else(|| ApiError::field(name, "Operations must be an array."))?;
        for operation in operations {
            let operation = operation
                .as_object()
                .ok_or_else(|| ApiError::field(name, "Each operation must be an object."))?;
            for (verb, value) in operation {
                match (name.as_str(), verb.as_str()) {
                    ("comment", "add") => {
                        let body = value.get("body").unwrap_or(&Value::Null);
                        let stored = rich_text_input("comment", body)?.ok_or_else(|| {
                            ApiError::field("comment", "Comment body can not be empty!")
                        })?;
                        comments.push(serde_json::from_str(&stored).expect("stored ADF parses"));
                    }
                    (_, "set") => {
                        set_field(ctx, custom, project, target, name, value, mode).await?;
                    }
                    (_, "add" | "remove") => {
                        let db = ctx.state.db.clone();
                        apply_list_operation(
                            &db,
                            custom,
                            project,
                            target,
                            name,
                            verb == "add",
                            value,
                        )
                        .await?;
                    }
                    _ => {
                        return Err(ApiError::field(
                            name,
                            format!("Operation '{verb}' is not supported for field '{name}'."),
                        ));
                    }
                }
            }
        }
    }
    if target.summary.trim().is_empty() {
        return Err(ApiError::field(
            "summary",
            "You must specify a summary of the issue.",
        ));
    }
    check_hierarchy(ctx, target).await?;
    Ok(comments)
}

async fn set_field(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    project: &Project,
    target: &mut IssueFields,
    name: &str,
    value: &Value,
    mode: Mode,
) -> ApiResult<()> {
    match name {
        "summary" => {
            let summary = value
                .as_str()
                .ok_or_else(|| {
                    ApiError::field("summary", "You must specify a summary of the issue.")
                })?
                .trim();
            if summary.chars().count() > 255 {
                return Err(ApiError::field(
                    "summary",
                    "Summary must be less than 255 characters.",
                ));
            }
            target.summary = summary.to_string();
        }
        "description" => target.description = rich_text_input("description", value)?,
        "environment" => target.environment = rich_text_input("environment", value)?,
        "project" if mode == Mode::Create => {
            // The caller resolved the project already; this only checks that the reference agrees.
            let matches = value.get("key").and_then(Value::as_str) == Some(project.key.as_str())
                || value.get("id").and_then(json_id) == Some(project.id);
            if !matches {
                return Err(ApiError::field(
                    "project",
                    "Specify a valid project ID or key.",
                ));
            }
        }
        "issuetype" => {
            let issue_type = find_by_ref(&ctx.catalog.issue_types, value, |t| t.id, |t| &t.name)
                .ok_or_else(|| {
                    ApiError::field("issuetype", "The issue type selected is invalid.")
                })?;
            target.issue_type_id = issue_type.id;
        }
        "priority" => {
            target.priority_id = match value {
                Value::Null => None,
                _ => Some(
                    find_by_ref(&ctx.catalog.priorities, value, |p| p.id, |p| &p.name)
                        .ok_or_else(|| {
                            ApiError::field("priority", "The priority selected is invalid.")
                        })?
                        .id,
                ),
            };
        }
        "resolution" => {
            target.resolution_id = match value {
                Value::Null => None,
                _ => Some(
                    find_by_ref(&ctx.catalog.resolutions, value, |r| r.id, |r| &r.name)
                        .ok_or_else(|| {
                            ApiError::field("resolution", "The resolution selected is invalid.")
                        })?
                        .id,
                ),
            };
        }
        "assignee" => target.assignee_id = user_ref(&ctx.state.db, "assignee", value).await?,
        "reporter" => target.reporter_id = user_ref(&ctx.state.db, "reporter", value).await?,
        "labels" => target.labels = labels("labels", value)?,
        "components" | "fixVersions" | "versions" => {
            let part = Part::from_name(name).expect("matched above");
            *part.list_mut(target) = part_ids(&ctx.state.db, project.id, part, value).await?;
        }
        "timetracking" => set_time_tracking(target, value)?,
        "duedate" => {
            target.due_date = match value {
                Value::Null => None,
                Value::String(text) if text.is_empty() => None,
                Value::String(text) if time::parse_date(text).is_some() => Some(text.clone()),
                other => {
                    return Err(ApiError::field(
                        "duedate",
                        format!("Error parsing date string: {other}"),
                    ));
                }
            };
        }
        "parent" => {
            target.parent_id = match value {
                Value::Null => None,
                _ => {
                    let id_or_key = value
                        .get("key")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| value.get("id").and_then(json_id).map(|id| id.to_string()))
                        .ok_or_else(|| {
                            ApiError::field("parent", "Specify a valid parent issue.")
                        })?;
                    let parent = issues::by_id_or_key(&ctx.state.db, &id_or_key)
                        .await?
                        .filter(|p| p.project_id == project.id)
                        .ok_or_else(|| {
                            ApiError::field(
                                "parent",
                                format!("Could not find issue by id or key: {id_or_key}"),
                            )
                        })?;
                    Some(parent.id)
                }
            };
        }
        key if key.starts_with("customfield_") => {
            let field = custom.by_key(key).ok_or_else(|| cannot_set(key))?;
            let stored = custom_value(ctx, custom, field, value).await?;
            set_custom(target, field.id, stored);
        }
        // Read-only fields, and fields Tessera does not have yet.
        other => return Err(cannot_set(other)),
    }
    Ok(())
}

/// The list fields that refer to project parts: components and versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Components,
    FixVersions,
    AffectsVersions,
}

impl Part {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "components" => Some(Self::Components),
            "fixVersions" => Some(Self::FixVersions),
            "versions" => Some(Self::AffectsVersions),
            _ => None,
        }
    }

    fn list_mut(self, fields: &mut IssueFields) -> &mut Vec<i64> {
        match self {
            Self::Components => &mut fields.components,
            Self::FixVersions => &mut fields.fix_versions,
            Self::AffectsVersions => &mut fields.affects_versions,
        }
    }

    fn list(self, fields: &IssueFields) -> &[i64] {
        match self {
            Self::Components => &fields.components,
            Self::FixVersions => &fields.fix_versions,
            Self::AffectsVersions => &fields.affects_versions,
        }
    }

    /// The field id, and the name Jira's history uses for it.
    fn history_names(self) -> (&'static str, &'static str) {
        match self {
            Self::Components => ("components", "Component"),
            Self::FixVersions => ("fixVersions", "Fix Version"),
            Self::AffectsVersions => ("versions", "Version"),
        }
    }
}

/// Resolves `[{"id": ...}, {"name": ...}]` to component or version ids in the project.
async fn part_ids(
    db: &SqlitePool,
    project_id: i64,
    part: Part,
    value: &Value,
) -> ApiResult<Vec<i64>> {
    let (field, _) = part.history_names();
    let items = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        _ => return Err(ApiError::field(field, "Operation value must be an array.")),
    };
    // (id, name) pairs of the project's components or versions.
    let available: Vec<(i64, String)> = match part {
        Part::Components => components::for_project(db, project_id)
            .await?
            .into_iter()
            .map(|c| (c.id, c.name))
            .collect(),
        Part::FixVersions | Part::AffectsVersions => versions::for_project(db, project_id)
            .await?
            .into_iter()
            .map(|v| (v.id, v.name))
            .collect(),
    };
    let what = if part == Part::Components {
        "Component"
    } else {
        "Version"
    };
    let mut ids = Vec::new();
    for item in items {
        let found = if let Some(id) = item.get("id").and_then(json_id) {
            available
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .ok_or_else(|| {
                    ApiError::field(field, format!("{what} with id '{id}' does not exist."))
                })?
        } else {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::field(field, format!("{what} id or name is required.")))?;
            available
                .iter()
                .find(|(_, candidate)| candidate.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    ApiError::field(field, format!("{what} name '{name}' is not valid"))
                })?
        };
        if !ids.contains(&found.0) {
            ids.push(found.0);
        }
    }
    Ok(ids)
}

/// `timetracking`: `{"originalEstimate": "1d", "remainingEstimate": "4h"}`. Setting only the
/// original estimate on an issue without a remaining estimate sets both, as in Jira.
fn set_time_tracking(target: &mut IssueFields, value: &Value) -> ApiResult<()> {
    let estimate = |key: &str| -> ApiResult<Option<Option<i64>>> {
        match value.get(key) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(Some(None)),
            Some(Value::String(text)) => time::parse_duration(text).map(|s| Some(Some(s))).ok_or_else(|| {
                ApiError::field(
                    "timetracking",
                    format!("Invalid time duration entered: '{text}'. Use a format such as '2w 4d 6h 45m'."),
                )
            }),
            Some(_) => Err(ApiError::field("timetracking", "Estimates must be strings such as '3h'.")),
        }
    };
    let original = estimate("originalEstimate")?;
    let remaining = estimate("remainingEstimate")?;
    if let Some(original) = original {
        target.original_estimate = original;
        if remaining.is_none() && target.remaining_estimate.is_none() {
            target.remaining_estimate = original;
        }
    }
    if let Some(remaining) = remaining {
        target.remaining_estimate = remaining;
    }
    Ok(())
}

/// Resolves a user reference. Jira Cloud clients send `{"accountId": ...}`; older ones send
/// `{"name": ...}` or `{"emailAddress": ...}`. `null`, or the account id `-1`, means nobody.
async fn user_ref(db: &SqlitePool, field: &str, value: &Value) -> ApiResult<Option<i64>> {
    if value.is_null() {
        return Ok(None);
    }
    let not_found = || {
        ApiError::field(
            field,
            "Specified user does not exist or you do not have required permissions",
        )
    };
    let account_id = value
        .get("accountId")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str);
    let user = match account_id {
        Some("-1") => return Ok(None),
        Some(account_id) => users::by_account_id(db, account_id).await?,
        None => {
            let email = value
                .get("emailAddress")
                .or_else(|| value.get("name"))
                .and_then(Value::as_str)
                .ok_or_else(not_found)?;
            users::by_email(db, email).await?
        }
    };
    Ok(Some(user.filter(|u| u.active).ok_or_else(not_found)?.id))
}

/// Labels: an array of strings without spaces.
fn labels(field: &str, value: &Value) -> ApiResult<Vec<String>> {
    let mut labels = Vec::new();
    for item in value.as_array().map(Vec::as_slice).unwrap_or_default() {
        let label = item
            .as_str()
            .ok_or_else(|| ApiError::field(field, "Labels must be strings."))?;
        if label.contains(char::is_whitespace) {
            return Err(ApiError::field(
                field,
                format!("The label '{label}' contains spaces which is invalid."),
            ));
        }
        if label.chars().count() > 255 {
            return Err(ApiError::field(
                field,
                "The label must not exceed 255 characters.",
            ));
        }
        if !labels.iter().any(|l: &String| l == label) {
            labels.push(label.to_string());
        }
    }
    labels.sort();
    Ok(labels)
}

/// `add` and `remove` operations on list fields.
async fn apply_list_operation(
    db: &SqlitePool,
    custom: &CustomCatalog,
    project: &Project,
    target: &mut IssueFields,
    name: &str,
    add: bool,
    value: &Value,
) -> ApiResult<()> {
    if let Some(part) = Part::from_name(name) {
        let ids = part_ids(db, project.id, part, &Value::Array(vec![value.clone()])).await?;
        let list = part.list_mut(target);
        for id in ids {
            list.retain(|existing| *existing != id);
            if add {
                list.push(id);
            }
        }
        return Ok(());
    }
    if name == "labels" {
        let label = value
            .as_str()
            .ok_or_else(|| ApiError::field("labels", "Labels must be strings."))?;
        if add {
            let mut updated = target.labels.clone();
            updated.push(label.to_string());
            target.labels = labels("labels", &json!(updated))?;
        } else {
            target.labels.retain(|l| l != label);
        }
        return Ok(());
    }
    let field = custom.by_key(name).ok_or_else(|| cannot_set(name))?;
    if !matches!(field.field_type.as_str(), "labels" | "options") {
        return Err(ApiError::field(
            name,
            "This field does not support add or remove.",
        ));
    }
    // Start from the pending value if this request already set it, else from nothing. Editing
    // loads current custom values into `custom_values` first, so this sees the stored list.
    let current = target
        .custom_values
        .iter()
        .find(|(id, _)| *id == field.id)
        .and_then(|(_, v)| v.as_deref())
        .and_then(|v| serde_json::from_str::<Vec<Value>>(v).ok())
        .unwrap_or_default();
    let item = if field.field_type == "options" {
        let option = custom
            .option_by_ref(field.id, value)
            .ok_or_else(|| ApiError::field(name, "The option is not valid for this field."))?;
        json!({ "id": option.id.to_string() })
    } else {
        value.clone()
    };
    let mut updated: Vec<Value> = current.into_iter().filter(|v| *v != item).collect();
    if add {
        updated.push(item);
    }
    let stored = (!updated.is_empty()).then(|| Value::Array(updated).to_string());
    set_custom(target, field.id, stored);
    Ok(())
}

fn set_custom(target: &mut IssueFields, field_id: i64, stored: Option<String>) {
    target.custom_values.retain(|(id, _)| *id != field_id);
    target.custom_values.push((field_id, stored));
}

/// Validates a custom field value and converts it to the stored JSON form.
async fn custom_value(
    ctx: &Ctx,
    custom: &CustomCatalog,
    field: &CustomField,
    value: &Value,
) -> ApiResult<Option<String>> {
    let key = super::fields::custom_field_key(field.id);
    if value.is_null() || value.as_str().is_some_and(str::is_empty) {
        return Ok(None);
    }
    let invalid = |what: &str| ApiError::field(&key, format!("Operation value must be {what}"));
    let stored = match field.field_type.as_str() {
        "string" | "url" => json!(value.as_str().ok_or_else(|| invalid("a string"))?),
        "text" => {
            let adf = rich_text_input(&key, value)?;
            return Ok(adf);
        }
        "number" => json!(value.as_f64().ok_or_else(|| invalid("a number"))?),
        "date" => {
            let text = value.as_str().filter(|t| time::parse_date(t).is_some());
            json!(text.ok_or_else(|| invalid("a date in the format yyyy-MM-dd"))?)
        }
        "datetime" => json!(
            value
                .as_str()
                .ok_or_else(|| invalid("a date-time string"))?
        ),
        "user" => {
            let user_id = user_ref(&ctx.state.db, &key, value)
                .await?
                .ok_or_else(|| invalid("a user"))?;
            let user = users::by_id(&ctx.state.db, user_id)
                .await?
                .expect("user exists");
            json!({ "accountId": user.account_id })
        }
        "option" => {
            let option = custom
                .option_by_ref(field.id, value)
                .ok_or_else(|| ApiError::field(&key, "The option is not valid for this field."))?;
            json!({ "id": option.id.to_string() })
        }
        "options" => {
            let mut ids = Vec::new();
            for item in value.as_array().ok_or_else(|| invalid("an array"))? {
                let option = custom.option_by_ref(field.id, item).ok_or_else(|| {
                    ApiError::field(&key, "The option is not valid for this field.")
                })?;
                ids.push(json!({ "id": option.id.to_string() }));
            }
            json!(ids)
        }
        "labels" => json!(labels(&key, value)?),
        _ => return Err(invalid("a supported value")),
    };
    Ok(Some(stored.to_string()))
}

/// Renders a stored custom field value in the REST API's shape.
pub async fn render_custom_value(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    field: &CustomField,
    stored: &str,
) -> ApiResult<Value> {
    if field.field_type == "text" {
        return Ok(ctx.rich_text(Some(stored)));
    }
    let value: Value = serde_json::from_str(stored).unwrap_or(Value::Null);
    let option_json = |ctx: &Ctx, reference: &Value| {
        reference
            .get("id")
            .and_then(json_id)
            .and_then(|id| custom.option(field.id, id))
            .map(|o| {
                json!({
                    "self": ctx.api_url(&format!("/customFieldOption/{}", o.id)),
                    "value": o.value,
                    "id": o.id.to_string(),
                })
            })
            .unwrap_or(Value::Null)
    };
    Ok(match field.field_type.as_str() {
        "user" => {
            let account_id = value
                .get("accountId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match users::by_account_id(&ctx.state.db, account_id).await? {
                Some(user) => ctx.user(&user),
                None => Value::Null,
            }
        }
        "option" => option_json(ctx, &value),
        "options" => Value::Array(
            value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|v| option_json(ctx, v))
                .collect(),
        ),
        _ => value,
    })
}

/// Human-readable text of a stored custom field value, for history entries.
fn custom_value_text(custom: &CustomCatalog, field: &CustomField, stored: &str) -> String {
    if field.field_type == "text" {
        return tessera_adf_text(stored);
    }
    let value: Value = serde_json::from_str(stored).unwrap_or(Value::Null);
    let option_text = |v: &Value| {
        v.get("id")
            .and_then(json_id)
            .and_then(|id| custom.option(field.id, id))
            .map(|o| o.value.clone())
            .unwrap_or_default()
    };
    match (field.field_type.as_str(), &value) {
        ("option", v) => option_text(v),
        ("options", Value::Array(items)) => {
            items.iter().map(option_text).collect::<Vec<_>>().join(",")
        }
        ("labels", Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        ("user", v) => v
            .get("accountId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        (_, Value::String(text)) => text.clone(),
        (_, other) => other.to_string(),
    }
}

fn tessera_adf_text(stored: &str) -> String {
    serde_json::from_str::<tessera_adf::Node>(stored)
        .map(|doc| tessera_adf::to_plain_text(&doc))
        .unwrap_or_default()
}

/// Jira's issue hierarchy: subtasks need a parent that is not a subtask; standard issues may
/// have an epic as parent; epics have no parent.
async fn check_hierarchy(ctx: &Ctx, target: &IssueFields) -> ApiResult<()> {
    let issue_type = ctx
        .catalog
        .issue_type(target.issue_type_id)
        .ok_or_else(|| ApiError::field("issuetype", "The issue type selected is invalid."))?;
    let parent_type = match target.parent_id {
        Some(parent_id) => {
            let parent = issues::by_id(&ctx.state.db, parent_id)
                .await?
                .ok_or_else(|| ApiError::field("parent", "Specify a valid parent issue."))?;
            ctx.catalog.issue_type(parent.issue_type_id)
        }
        None => None,
    };
    let valid = match (
        issue_type.hierarchy_level,
        parent_type.map(|t| t.hierarchy_level),
    ) {
        (-1, Some(level)) => level >= 0,
        (-1, None) => {
            return Err(ApiError::field(
                "parent",
                "Given parent work item does not belong to appropriate hierarchy.",
            ));
        }
        (0, Some(level)) => level == 1,
        (_, None) => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::field(
            "parent",
            "Given parent work item does not belong to appropriate hierarchy.",
        ))
    }
}

/// Compares two versions of an issue and describes each change as Jira's history does.
pub async fn diff(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    before: &IssueFields,
    before_custom: &[(i64, String)],
    after: &IssueFields,
) -> ApiResult<Vec<ChangeItem>> {
    let mut items = Vec::new();
    let mut push = |field: &str,
                    from: (Option<String>, Option<String>),
                    to: (Option<String>, Option<String>)| {
        items.push(ChangeItem {
            field: field.to_string(),
            field_id: field.to_string(),
            field_type: "jira".into(),
            from_value: from.0,
            from_string: from.1,
            to_value: to.0,
            to_string: to.1,
        });
    };
    let text = |s: &str| (None, Some(s.to_string()));
    let rich = |adf: &Option<String>| (None, adf.as_deref().map(tessera_adf_text));

    if before.summary != after.summary {
        push("summary", text(&before.summary), text(&after.summary));
    }
    if before.description != after.description {
        push(
            "description",
            rich(&before.description),
            rich(&after.description),
        );
    }
    if before.environment != after.environment {
        push(
            "environment",
            rich(&before.environment),
            rich(&after.environment),
        );
    }
    let catalog = &ctx.catalog;
    let named = |id: Option<i64>, name: Option<&str>| {
        (id.map(|id| id.to_string()), name.map(str::to_string))
    };
    if before.issue_type_id != after.issue_type_id {
        let name = |id| catalog.issue_type(id).map(|t| t.name.as_str());
        push(
            "issuetype",
            named(Some(before.issue_type_id), name(before.issue_type_id)),
            named(Some(after.issue_type_id), name(after.issue_type_id)),
        );
    }
    if before.status_id != after.status_id {
        let name = |id| catalog.status(id).map(|s| s.name.as_str());
        push(
            "status",
            named(Some(before.status_id), name(before.status_id)),
            named(Some(after.status_id), name(after.status_id)),
        );
    }
    if before.priority_id != after.priority_id {
        let name = |id: Option<i64>| {
            id.and_then(|id| catalog.priority(id))
                .map(|p| p.name.as_str())
        };
        push(
            "priority",
            named(before.priority_id, name(before.priority_id)),
            named(after.priority_id, name(after.priority_id)),
        );
    }
    if before.resolution_id != after.resolution_id {
        let name = |id: Option<i64>| {
            id.and_then(|id| catalog.resolution(id))
                .map(|r| r.name.as_str())
        };
        push(
            "resolution",
            named(before.resolution_id, name(before.resolution_id)),
            named(after.resolution_id, name(after.resolution_id)),
        );
    }
    if before.due_date != after.due_date {
        push(
            "duedate",
            (before.due_date.clone(), before.due_date.clone()),
            (after.due_date.clone(), after.due_date.clone()),
        );
    }
    if before.labels != after.labels {
        push(
            "labels",
            text(&before.labels.join(" ")),
            text(&after.labels.join(" ")),
        );
    }
    // User fields record account ids and display names.
    for (field, from, to) in [
        ("assignee", before.assignee_id, after.assignee_id),
        ("reporter", before.reporter_id, after.reporter_id),
    ] {
        if from != to {
            let from = user_change(ctx, from).await?;
            let to = user_change(ctx, to).await?;
            items.push(ChangeItem {
                field: field.into(),
                field_id: field.into(),
                field_type: "jira".into(),
                from_value: from.0,
                from_string: from.1,
                to_value: to.0,
                to_string: to.1,
            });
        }
    }
    if before.parent_id != after.parent_id {
        let from = issue_key(ctx, before.parent_id).await?;
        let to = issue_key(ctx, after.parent_id).await?;
        items.push(ChangeItem {
            field: "IssueParentAssociation".into(),
            field_id: "parent".into(),
            field_type: "jira".into(),
            from_value: before.parent_id.map(|id| id.to_string()),
            from_string: from,
            to_value: after.parent_id.map(|id| id.to_string()),
            to_string: to,
        });
    }
    // List fields record one entry per added or removed value.
    for part in [Part::Components, Part::FixVersions, Part::AffectsVersions] {
        let (field_id, field_name) = part.history_names();
        let (old, new) = (part.list(before), part.list(after));
        for (id, added) in old
            .iter()
            .filter(|id| !new.contains(id))
            .map(|id| (*id, false))
            .chain(
                new.iter()
                    .filter(|id| !old.contains(id))
                    .map(|id| (*id, true)),
            )
        {
            let name = part_name(&ctx.state.db, part, id).await?;
            let (value, text) = (Some(id.to_string()), Some(name));
            items.push(ChangeItem {
                field: field_name.into(),
                field_id: field_id.into(),
                field_type: "jira".into(),
                from_value: if added { None } else { value.clone() },
                from_string: if added { None } else { text.clone() },
                to_value: if added { value } else { None },
                to_string: if added { text } else { None },
            });
        }
    }
    for (field, from, to) in [
        (
            "timeoriginalestimate",
            before.original_estimate,
            after.original_estimate,
        ),
        (
            "timeestimate",
            before.remaining_estimate,
            after.remaining_estimate,
        ),
    ] {
        if from != to {
            let seconds = |v: Option<i64>| v.map(|s| s.to_string());
            items.push(ChangeItem {
                field: field.into(),
                field_id: field.into(),
                field_type: "jira".into(),
                from_value: seconds(from),
                from_string: seconds(from),
                to_value: seconds(to),
                to_string: seconds(to),
            });
        }
    }
    for (field_id, new_value) in &after.custom_values {
        let old_value = before_custom
            .iter()
            .find(|(id, _)| id == field_id)
            .map(|(_, v)| v.clone());
        if old_value.as_ref() == new_value.as_ref() {
            continue;
        }
        let Some(field) = custom.fields.iter().find(|f| f.id == *field_id) else {
            continue;
        };
        let render = |v: &Option<String>| v.as_deref().map(|v| custom_value_text(custom, field, v));
        items.push(ChangeItem {
            field: field.name.clone(),
            field_id: super::fields::custom_field_key(field.id),
            field_type: "custom".into(),
            from_value: None,
            from_string: render(&old_value),
            to_value: None,
            to_string: render(new_value),
        });
    }
    Ok(items)
}

async fn user_change(
    ctx: &mut Ctx,
    id: Option<i64>,
) -> ApiResult<(Option<String>, Option<String>)> {
    let Some(id) = id else {
        return Ok((None, None));
    };
    Ok(match ctx.load_user(id).await? {
        Some(user) => (Some(user.account_id), Some(user.display_name)),
        None => (None, None),
    })
}

async fn issue_key(ctx: &Ctx, id: Option<i64>) -> ApiResult<Option<String>> {
    let Some(id) = id else {
        return Ok(None);
    };
    Ok(issues::by_id(&ctx.state.db, id).await?.map(|i| i.key))
}

async fn part_name(db: &SqlitePool, part: Part, id: i64) -> ApiResult<String> {
    Ok(match part {
        Part::Components => components::by_id(db, id).await?.map(|c| c.name),
        Part::FixVersions | Part::AffectsVersions => versions::by_id(db, id).await?.map(|v| v.name),
    }
    .unwrap_or_default())
}
