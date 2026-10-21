//! Bulk operations on issues.
//!
//! - `POST /issue/bulk`: creates many issues at once, as Jira Server and Cloud both do.
//! - `/bulk/issues/fields`, `/transition`, `/delete`, `/move`, `/watch`, and `/unwatch`: Jira
//!   Cloud's bulk change. Each answers 201 with a task id; `/bulk/queue/{taskId}` reports
//!   which issues were changed and why others failed. Tessera does the work before answering,
//!   so a task is always complete when it is read.
//!
//! Each issue goes through the same checks as a change to that one issue.

use std::collections::{BTreeMap, HashMap};

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::ApiVersion;
use super::access::{Access, find_issue_as};
use super::extract::Json;
use super::issues::{IssueRequest, create_issue, delete_issue, save_edit};
use super::render::Ctx;
use super::transitions;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::notify::{self, IssueEvent};
use crate::store::issues::{self, ChangeItem, Issue, Relocation};
use crate::store::meta::Catalog;
use crate::store::social::{self, List};
use crate::store::users::User;
use crate::store::{bulk, projects, security, workflows};
use crate::time::format;

/// The most issues one bulk operation may change, as in Jira.
const MAX_ISSUES: usize = 1000;

/// What came of a bulk operation.
#[derive(Default)]
struct Outcome {
    total: usize,
    processed: Vec<i64>,
    failed: BTreeMap<i64, Vec<String>>,
    /// Issues that do not exist, or that the user cannot see.
    invalid: usize,
}

fn messages(error: &ApiError) -> Vec<String> {
    let mut out = error.error_messages.clone();
    out.extend(
        error
            .errors
            .iter()
            .map(|(field, message)| format!("{field}: {message}")),
    );
    out
}

impl Outcome {
    /// Records the result of one issue's change.
    fn record(&mut self, issue: &Issue, result: ApiResult<()>) {
        match result {
            Ok(()) => self.processed.push(issue.id),
            Err(error) => {
                self.failed.insert(issue.id, messages(&error));
            }
        }
    }

    /// Records an issue that could not be loaded with the permission the change needs: a
    /// failure when the user can see it, otherwise an invalid or inaccessible issue.
    fn unavailable(&mut self, error: ApiError, existing: Option<Issue>) {
        match (error.status, existing) {
            (StatusCode::FORBIDDEN, Some(issue)) => {
                self.failed.insert(issue.id, messages(&error));
            }
            _ => self.invalid += 1,
        }
    }

    async fn save(self, state: &AppState, user: &User) -> ApiResult<(StatusCode, Json<Value>)> {
        let failed: Map<String, Value> = self
            .failed
            .iter()
            .map(|(id, errors)| (id.to_string(), json!(errors)))
            .collect();
        let result = json!({
            "totalIssueCount": self.total,
            "processedAccessibleIssues": self.processed,
            "failedAccessibleIssues": failed,
            "invalidOrInaccessibleIssueCount": self.invalid,
        });
        let id = bulk::record(&state.db, user.id, &result).await?;
        Ok((
            StatusCode::CREATED,
            Json(json!({ "taskId": id.to_string() })),
        ))
    }
}

fn check_size(refs: &[String]) -> ApiResult<()> {
    if refs.is_empty() {
        return Err(ApiError::bad_request("Select at least one issue."));
    }
    if refs.len() > MAX_ISSUES {
        return Err(ApiError::bad_request(format!(
            "A bulk operation changes at most {MAX_ISSUES} issues."
        )));
    }
    Ok(())
}

/// The issues named, each loaded with `permission`, or recorded as unavailable.
async fn load(
    state: &AppState,
    user: &User,
    refs: &[String],
    permission: &str,
    outcome: &mut Outcome,
) -> ApiResult<Vec<Issue>> {
    let mut found = Vec::new();
    for issue_ref in refs {
        outcome.total += 1;
        match find_issue_as(state, user, issue_ref, permission).await {
            Ok(issue) => found.push(issue),
            Err(error) => {
                let existing = issues::by_id_or_key(&state.db, issue_ref).await?;
                outcome.unavailable(error, existing);
            }
        }
    }
    Ok(found)
}

// Bulk create.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkCreateRequest {
    issue_updates: Vec<IssueRequest>,
}

/// `POST /issue/bulk`: creates each issue in turn. Issues that fail are reported with their
/// position, and the others are still created, as in Jira.
pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<BulkCreateRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if request.issue_updates.len() > 50 {
        return Err(ApiError::bad_request(
            "Bulk create accepts at most 50 issues at a time.",
        ));
    }
    let ctx = Ctx::new(&state, version).await?;
    let mut created = Vec::new();
    let mut errors = Vec::new();
    for (index, item) in request.issue_updates.iter().enumerate() {
        match create_issue(&state, version, &user, item).await {
            Ok(issue) => created.push(json!({
                "id": issue.id.to_string(),
                "key": issue.key,
                "self": ctx.api_url(&format!("/issue/{}", issue.id)),
            })),
            Err(error) => errors.push(json!({
                "status": error.status.as_u16(),
                "elementErrors": { "errorMessages": error.error_messages, "errors": error.errors },
                "failedElementNumber": index,
            })),
        }
    }
    // Jira answers 201 when every issue was created, and 400 when none was.
    let status = if created.is_empty() && !errors.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(json!({ "issues": created, "errors": errors }))))
}

// Bulk edit.

/// A list change: `ADD`, `REMOVE`, `REPLACE`, or `REMOVE_ALL`.
fn list_update(
    field: &str,
    option: &str,
    values: Vec<Value>,
) -> ApiResult<(Option<Value>, Option<Value>)> {
    Ok(match option {
        "ADD" => (
            None,
            Some(json!(
                values
                    .into_iter()
                    .map(|v| json!({ "add": v }))
                    .collect::<Vec<_>>()
            )),
        ),
        "REMOVE" => (
            None,
            Some(json!(
                values
                    .into_iter()
                    .map(|v| json!({ "remove": v }))
                    .collect::<Vec<_>>()
            )),
        ),
        "REPLACE" => (Some(json!(values)), None),
        "REMOVE_ALL" => (Some(json!([])), None),
        other => {
            return Err(ApiError::bad_request(format!(
                "'{other}' is not a valid change for {field}. Use ADD, REMOVE, REPLACE, or REMOVE_ALL."
            )));
        }
    })
}

/// Turns Jira Cloud's `editedFieldsInput` into an edit request, keeping only `selectedActions`.
fn edit_request(edited: &Value, selected: &[String]) -> ApiResult<IssueRequest> {
    let mut fields = Map::new();
    let mut update = Map::new();
    let wanted = |field: &str| selected.iter().any(|s| s == field);
    let mut apply = |field: &str, (set, ops): (Option<Value>, Option<Value>)| {
        if let Some(set) = set {
            fields.insert(field.to_string(), set);
        }
        if let Some(ops) = ops {
            update.insert(field.to_string(), ops);
        }
    };
    if wanted("priority")
        && let Some(id) = edited["priority"]["priorityId"].as_str()
    {
        apply("priority", (Some(json!({ "id": id })), None));
    }
    if wanted("issuetype")
        && let Some(id) = edited["issueType"]["issueTypeId"].as_str()
    {
        apply("issuetype", (Some(json!({ "id": id })), None));
    }
    for item in edited["singleSelectClearableUserPickerFields"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let field = item["fieldId"].as_str().unwrap_or_default();
        if matches!(field, "assignee" | "reporter") && wanted(field) {
            let user = match item["user"]["accountId"].as_str() {
                Some(account) => json!({ "accountId": account }),
                None => Value::Null,
            };
            apply(field, (Some(user), None));
        }
    }
    for item in edited["labelsFields"].as_array().into_iter().flatten() {
        if wanted("labels") {
            let labels = item["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l["name"].as_str().map(|n| json!(n)))
                .collect();
            let option = item["bulkEditMultiSelectFieldOption"]
                .as_str()
                .unwrap_or("ADD");
            apply("labels", list_update("labels", option, labels)?);
        }
    }
    let components = &edited["multiselectComponents"];
    if wanted("components") && !components.is_null() {
        let ids = components["components"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                c["componentId"]
                    .as_i64()
                    .map(|id| json!({ "id": id.to_string() }))
                    .or_else(|| c["componentId"].as_str().map(|id| json!({ "id": id })))
            })
            .collect();
        let option = components["bulkEditMultiSelectFieldOption"]
            .as_str()
            .unwrap_or("ADD");
        apply("components", list_update("components", option, ids)?);
    }
    for item in edited["multipleVersionPickerFields"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let field = item["fieldId"].as_str().unwrap_or_default();
        if matches!(field, "fixVersions" | "versions") && wanted(field) {
            let ids = item["versionIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(|id| json!({ "id": id })))
                .collect();
            let option = item["bulkEditMultiSelectFieldOption"]
                .as_str()
                .unwrap_or("ADD");
            apply(field, list_update(field, option, ids)?);
        }
    }
    for item in edited["datePickerFields"].as_array().into_iter().flatten() {
        if item["fieldId"] == "duedate" && wanted("duedate") {
            let date = item["date"]["formattedDate"].as_str().map(|d| json!(d));
            apply("duedate", (Some(date.unwrap_or(Value::Null)), None));
        }
    }
    if fields.is_empty() && update.is_empty() {
        return Err(ApiError::bad_request(
            "Pick at least one field to change, with a value, in selectedActions and editedFieldsInput.",
        ));
    }
    Ok(IssueRequest { fields, update })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkEditRequest {
    selected_issue_ids_or_keys: Vec<String>,
    selected_actions: Vec<String>,
    edited_fields_input: Value,
    #[serde(default = "yes")]
    send_bulk_notification: bool,
}

fn yes() -> bool {
    true
}

/// `POST /bulk/issues/fields`: the same field changes on every issue.
pub async fn edit(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<BulkEditRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    check_size(&request.selected_issue_ids_or_keys)?;
    let edit = edit_request(&request.edited_fields_input, &request.selected_actions)?;
    let mut outcome = Outcome::default();
    let found = load(
        &state,
        &user,
        &request.selected_issue_ids_or_keys,
        "EDIT_ISSUES",
        &mut outcome,
    )
    .await?;
    let mut ctx = Ctx::new(&state, version).await?;
    for issue in found {
        let result = save_edit(&mut ctx, &issue, user.id, &edit).await;
        if let Ok(changed) = &result
            && request.send_bulk_notification
            && !changed.is_empty()
            && let Some(now) = issues::by_id(&state.db, issue.id).await?
        {
            notify::issue(&state, &user, &now, IssueEvent::Updated(changed)).await;
        }
        outcome.record(&issue, result.map(|_| ()));
    }
    outcome.save(&state, &user).await
}

/// The fields bulk edit can change: (id, name, type, list changes allowed).
const EDITABLE: &[(&str, &str, &str, bool)] = &[
    ("assignee", "Assignee", "assignee", false),
    ("reporter", "Reporter", "reporter", false),
    ("priority", "Priority", "priority", false),
    ("issuetype", "Issue Type", "issuetype", false),
    ("labels", "Labels", "labels", true),
    ("components", "Components", "components", true),
    ("fixVersions", "Fix versions", "fixVersions", true),
    ("versions", "Affects versions", "versions", true),
    ("duedate", "Due date", "duedate", false),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuesQuery {
    issue_ids_or_keys: Option<String>,
}

fn refs_of(query: &IssuesQuery) -> Vec<String> {
    query
        .issue_ids_or_keys
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

/// `GET /bulk/issues/fields?issueIdsOrKeys=`: the fields bulk edit can change.
pub async fn editable_fields(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<IssuesQuery>,
) -> ApiResult<Json<Value>> {
    let refs = refs_of(&query);
    check_size(&refs)?;
    let mut outcome = Outcome::default();
    let found = load(&state, &user, &refs, "EDIT_ISSUES", &mut outcome).await?;
    let unavailable = if found.is_empty() {
        Some("You cannot edit any of the selected issues.")
    } else {
        None
    };
    let fields: Vec<Value> = EDITABLE
        .iter()
        .map(|(id, name, kind, multi)| {
            let mut field = json!({
                "id": id,
                "name": name,
                "type": kind,
                "isRequired": false,
                "searchable": false,
            });
            if *multi {
                field["multiSelectFieldOptions"] =
                    json!(["ADD", "REMOVE", "REPLACE", "REMOVE_ALL"]);
            }
            if let Some(message) = unavailable {
                field["unavailableMessage"] = json!(message);
            }
            field
        })
        .collect();
    Ok(Json(json!({ "fields": fields })))
}

// Bulk transition.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionInput {
    selected_issue_ids_or_keys: Vec<String>,
    transition_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkTransitionRequest {
    bulk_transition_inputs: Vec<TransitionInput>,
    #[serde(default = "yes")]
    send_bulk_notification: bool,
}

/// `POST /bulk/issues/transition`: each group of issues takes its transition.
pub async fn transition(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<BulkTransitionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let all: Vec<String> = request
        .bulk_transition_inputs
        .iter()
        .flat_map(|i| i.selected_issue_ids_or_keys.clone())
        .collect();
    check_size(&all)?;
    let mut outcome = Outcome::default();
    for input in &request.bulk_transition_inputs {
        let transition_id: i64 = input
            .transition_id
            .parse()
            .map_err(|_| ApiError::bad_request("transitionId must be a number."))?;
        let found = load(
            &state,
            &user,
            &input.selected_issue_ids_or_keys,
            "TRANSITION_ISSUES",
            &mut outcome,
        )
        .await?;
        for issue in found {
            let result = transitions::transition(
                &state,
                version,
                &user,
                &issue,
                transition_id,
                &IssueRequest::default(),
                request.send_bulk_notification,
            )
            .await;
            outcome.record(&issue, result);
        }
    }
    outcome.save(&state, &user).await
}

/// `GET /bulk/issues/transition?issueIdsOrKeys=`: the transitions available to the selected
/// issues, grouped by the workflow and status they share.
pub async fn available_transitions(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<IssuesQuery>,
) -> ApiResult<Json<Value>> {
    let refs = refs_of(&query);
    check_size(&refs)?;
    let mut outcome = Outcome::default();
    let found = load(&state, &user, &refs, "TRANSITION_ISSUES", &mut outcome).await?;
    let catalog = Catalog::load(&state.db).await?;
    let mut groups: BTreeMap<(i64, i64), Vec<String>> = BTreeMap::new();
    for issue in &found {
        let workflow =
            workflows::for_issue_type(&state.db, issue.project_id, issue.issue_type_id).await?;
        groups
            .entry((workflow, issue.status_id))
            .or_default()
            .push(issue.key.clone());
    }
    let mut out = Vec::new();
    for ((workflow, status), keys) in groups {
        let transitions: Vec<Value> = workflows::available(&state.db, workflow, status)
            .await?
            .iter()
            .map(|t| {
                json!({
                    "transitionId": t.id,
                    "transitionName": t.name,
                    "to": {
                        "statusId": t.to_status_id,
                        "statusName": catalog.status(t.to_status_id).map(|s| s.name.clone()),
                    },
                })
            })
            .collect();
        out.push(
            json!({ "issues": keys, "isTransitionsFiltered": false, "transitions": transitions }),
        );
    }
    Ok(Json(json!({ "availableTransitions": out })))
}

// Bulk delete, watch, and unwatch.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRequest {
    selected_issue_ids_or_keys: Vec<String>,
    #[serde(default = "yes")]
    send_bulk_notification: bool,
}

/// `POST /bulk/issues/delete`: deletes the issues and their subtasks.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SelectionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    check_size(&request.selected_issue_ids_or_keys)?;
    let mut outcome = Outcome::default();
    let found = load(
        &state,
        &user,
        &request.selected_issue_ids_or_keys,
        "DELETE_ISSUES",
        &mut outcome,
    )
    .await?;
    for issue in found {
        // A subtask may already be gone with its parent.
        if issues::by_id(&state.db, issue.id).await?.is_none() {
            outcome.processed.push(issue.id);
            continue;
        }
        let result =
            delete_issue(&state, &user, &issue, true, request.send_bulk_notification).await;
        outcome.record(&issue, result);
    }
    outcome.save(&state, &user).await
}

async fn set_watching(
    state: &AppState,
    user: &User,
    refs: &[String],
    watching: bool,
) -> ApiResult<(StatusCode, Json<Value>)> {
    check_size(refs)?;
    let mut outcome = Outcome::default();
    let found = load(state, user, refs, "BROWSE_PROJECTS", &mut outcome).await?;
    for issue in found {
        let result = if watching {
            social::add(&state.db, List::Watchers, issue.id, user.id).await
        } else {
            social::remove(&state.db, List::Watchers, issue.id, user.id).await
        };
        outcome.record(&issue, result.map_err(ApiError::from));
    }
    outcome.save(state, user).await
}

/// `POST /bulk/issues/watch`: the caller watches every issue.
pub async fn watch(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SelectionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    set_watching(&state, &user, &request.selected_issue_ids_or_keys, true).await
}

/// `POST /bulk/issues/unwatch`.
pub async fn unwatch(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SelectionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    set_watching(&state, &user, &request.selected_issue_ids_or_keys, false).await
}

// Bulk move.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveTarget {
    issue_ids_or_keys: Vec<String>,
    /// `[{"statuses": {"<target status id>": [{"statusIds": [<source status ids>]}]}}]`.
    #[serde(default)]
    target_status: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkMoveRequest {
    /// Keyed by `projectKeyOrId,issueTypeId`.
    target_to_sources_mapping: HashMap<String, MoveTarget>,
    #[serde(default = "yes")]
    send_bulk_notification: bool,
}

/// Source status id to target status id, from a `targetStatus` list.
fn status_mapping(target_status: &[Value]) -> HashMap<i64, i64> {
    let mut out = HashMap::new();
    for entry in target_status {
        for (target, sources) in entry["statuses"].as_object().into_iter().flatten() {
            let Ok(target) = target.parse::<i64>() else {
                continue;
            };
            for source in sources.as_array().into_iter().flatten() {
                for id in source["statusIds"].as_array().into_iter().flatten() {
                    if let Some(id) = id
                        .as_i64()
                        .or_else(|| id.as_str().and_then(|s| s.parse().ok()))
                    {
                        out.insert(id, target);
                    }
                }
            }
        }
    }
    out
}

fn change(
    field: &str,
    from: (Option<String>, Option<String>),
    to: (Option<String>, Option<String>),
) -> ChangeItem {
    ChangeItem {
        field: field.to_string(),
        field_id: field.to_string(),
        field_type: "jira".into(),
        from_value: from.0,
        from_string: from.1,
        to_value: to.0,
        to_string: to.1,
    }
}

/// Moves one issue, and its subtasks with it.
#[allow(clippy::too_many_arguments)]
async fn move_issue(
    state: &AppState,
    user: &User,
    catalog: &Catalog,
    issue: &Issue,
    target: &projects::Project,
    issue_type_id: i64,
    mapping: &HashMap<i64, i64>,
    send_notifications: bool,
) -> ApiResult<()> {
    // Moving out needs Move Issues here, and Create Issues where the issue goes.
    let source = projects::by_id(&state.db, issue.project_id)
        .await?
        .expect("issue has a project");
    let source_access = Access::load(&state.db, user, &source).await?;
    if !source_access.can_on("MOVE_ISSUES", Some(issue)) {
        return Err(ApiError::forbidden(
            "You do not have permission to move this issue.",
        ));
    }
    let target_access = Access::load(&state.db, user, target).await?;
    if !target_access.can("CREATE_ISSUES") {
        return Err(ApiError::forbidden(format!(
            "You do not have permission to create issues in {}.",
            target.key
        )));
    }
    let new_type = catalog.issue_type(issue_type_id).ok_or_else(|| {
        ApiError::bad_request(format!("The issue type {issue_type_id} does not exist."))
    })?;
    let old_type_is_subtask = catalog
        .issue_type(issue.issue_type_id)
        .is_some_and(|t| t.is_subtask());
    if new_type.is_subtask() != old_type_is_subtask {
        return Err(ApiError::bad_request(
            "Subtasks move as subtasks, and other issues as other issues.",
        ));
    }
    if old_type_is_subtask && target.id != issue.project_id {
        return Err(ApiError::bad_request(
            "A subtask moves with its parent. Move the parent instead.",
        ));
    }

    // The status: as mapped, the same one if the new workflow has it, or one of its kind.
    let workflow = workflows::for_issue_type(&state.db, target.id, issue_type_id).await?;
    let allowed = workflows::statuses(&state.db, workflow).await?;
    let status_for = |current: i64| -> i64 {
        if let Some(mapped) = mapping.get(&current).filter(|m| allowed.contains(m)) {
            return *mapped;
        }
        if allowed.contains(&current) {
            return current;
        }
        let category = catalog.status(current).map(|s| s.category_id);
        allowed
            .iter()
            .copied()
            .find(|s| catalog.status(*s).map(|x| x.category_id) == category)
            .unwrap_or_else(|| allowed.first().copied().unwrap_or(current))
    };
    // The level: kept when both projects share a scheme, else the new scheme's default.
    let source_scheme = security::project_scheme(&state.db, issue.project_id).await?;
    let target_scheme = security::project_scheme(&state.db, target.id).await?;
    let level = if source_scheme == target_scheme {
        issue.security_level_id
    } else {
        match target_scheme {
            Some(scheme) => security::scheme(&state.db, scheme)
                .await?
                .and_then(|s| s.default_level_id),
            None => None,
        }
    };

    let status = status_for(issue.status_id);
    let mut changes = Vec::new();
    if target.id != issue.project_id {
        changes.push(change(
            "project",
            (Some(source.id.to_string()), Some(source.name.clone())),
            (Some(target.id.to_string()), Some(target.name.clone())),
        ));
    }
    if issue_type_id != issue.issue_type_id {
        let name = |id| catalog.issue_type(id).map(|t| t.name.clone());
        changes.push(change(
            "issuetype",
            (
                Some(issue.issue_type_id.to_string()),
                name(issue.issue_type_id),
            ),
            (Some(issue_type_id.to_string()), name(issue_type_id)),
        ));
    }
    if status != issue.status_id {
        let name = |id| catalog.status(id).map(|s| s.name.clone());
        changes.push(change(
            "status",
            (Some(issue.status_id.to_string()), name(issue.status_id)),
            (Some(status.to_string()), name(status)),
        ));
    }
    let relocation = Relocation {
        project_id: target.id,
        issue_type_id,
        status_id: status,
        security_level_id: level,
    };
    issues::relocate(&state.db, issue, user.id, &relocation, &changes).await?;

    // Subtasks follow their parent into the new project, keeping their types.
    if target.id != issue.project_id {
        for subtask in issues::subtasks(&state.db, issue.id).await? {
            let workflow =
                workflows::for_issue_type(&state.db, target.id, subtask.issue_type_id).await?;
            let allowed = workflows::statuses(&state.db, workflow).await?;
            let status = if allowed.contains(&subtask.status_id) {
                subtask.status_id
            } else {
                status_for(subtask.status_id)
            };
            let relocation = Relocation {
                project_id: target.id,
                issue_type_id: subtask.issue_type_id,
                status_id: status,
                security_level_id: level,
            };
            let change_project = change(
                "project",
                (Some(source.id.to_string()), Some(source.name.clone())),
                (Some(target.id.to_string()), Some(target.name.clone())),
            );
            issues::relocate(&state.db, &subtask, user.id, &relocation, &[change_project]).await?;
        }
    }
    if send_notifications && let Some(moved) = issues::by_id(&state.db, issue.id).await? {
        let changed = vec!["project".to_string()];
        notify::issue(state, user, &moved, IssueEvent::Updated(&changed)).await;
    }
    Ok(())
}

/// `POST /bulk/issues/move`: moves issues to other projects or issue types.
pub async fn move_issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<BulkMoveRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let all: Vec<String> = request
        .target_to_sources_mapping
        .values()
        .flat_map(|t| t.issue_ids_or_keys.clone())
        .collect();
    check_size(&all)?;
    let catalog = Catalog::load(&state.db).await?;
    let mut outcome = Outcome::default();
    for (target_key, target) in &request.target_to_sources_mapping {
        let mut parts = target_key.split(',');
        let project_ref = parts.next().unwrap_or_default().trim();
        let issue_type_id: i64 = parts
            .next()
            .and_then(|t| t.trim().parse().ok())
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "'{target_key}' must be a project key or id, a comma, and an issue type id."
                ))
            })?;
        let project = projects::by_id_or_key(&state.db, project_ref)
            .await?
            .ok_or_else(|| ApiError::project_not_found(project_ref))?;
        let mapping = status_mapping(&target.target_status);
        let found = load(
            &state,
            &user,
            &target.issue_ids_or_keys,
            "BROWSE_PROJECTS",
            &mut outcome,
        )
        .await?;
        for issue in found {
            let result = move_issue(
                &state,
                &user,
                &catalog,
                &issue,
                &project,
                issue_type_id,
                &mapping,
                request.send_bulk_notification,
            )
            .await;
            outcome.record(&issue, result);
        }
    }
    outcome.save(&state, &user).await
}

// Progress.

/// `GET /bulk/queue/{taskId}`: what a bulk operation did. Only its submitter may read it.
pub async fn progress(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let missing = || ApiError::not_found(format!("The bulk task {task_id} does not exist."));
    let id: i64 = task_id.parse().map_err(|_| missing())?;
    let task = bulk::by_id(&state.db, id)
        .await?
        .filter(|t| t.user_id == user.id || user.is_admin)
        .ok_or_else(missing)?;
    let mut out: Value = serde_json::from_str(&task.result).unwrap_or_else(|_| json!({}));
    let mut ctx = Ctx::new(&state, version).await?;
    let when = format(task.created_at);
    out["taskId"] = json!(task.id.to_string());
    out["status"] = json!("COMPLETE");
    out["progressPercent"] = json!(100);
    out["created"] = json!(when);
    out["started"] = json!(when);
    out["updated"] = json!(when);
    out["submittedBy"] = ctx.user_ref(Some(task.user_id)).await?;
    Ok(Json(out))
}
