//! Statuses, workflows, and workflow schemes: `/statuses`, `/workflow`, and `/workflowscheme`.
//!
//! Changing these is for site administrators, as in Jira.

use std::collections::{BTreeMap, HashSet};

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::extract::Json;
use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::meta::{self, Catalog, Status, json_id};
use crate::store::workflows::{self, NewTransition, Scheme, Workflow};
use crate::store::{boards, issues, projects};

/// Jira's status category names in the new statuses API, and the category ids behind them.
const CATEGORIES: &[(&str, i64)] = &[("TODO", 2), ("IN_PROGRESS", 4), ("DONE", 3)];

fn category_id(name: &str) -> ApiResult<i64> {
    CATEGORIES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, id)| *id)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "Unknown status category: {name}. Use TODO, IN_PROGRESS, or DONE."
            ))
        })
}

fn category_name(id: i64) -> &'static str {
    CATEGORIES
        .iter()
        .find(|(_, c)| *c == id)
        .map_or("TODO", |(n, _)| n)
}

fn status_json(status: &Status) -> Value {
    json!({
        "id": status.id.to_string(),
        "name": status.name,
        "description": status.description,
        "statusCategory": category_name(status.category_id),
        "scope": { "type": "GLOBAL" },
    })
}

// Statuses.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusInput {
    id: Option<Value>,
    name: Option<String>,
    status_category: Option<String>,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
pub struct StatusesRequest {
    statuses: Vec<StatusInput>,
}

/// `POST /statuses`: creates statuses.
pub async fn create_statuses(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<StatusesRequest>,
) -> ApiResult<Json<Value>> {
    let catalog = Catalog::load(&state.db).await?;
    let mut created = Vec::new();
    for input in &request.statuses {
        let name = input.name.as_deref().map(str::trim).unwrap_or_default();
        if name.is_empty() || name.chars().count() > 255 {
            return Err(ApiError::bad_request(
                "A status needs a name of 1 to 255 characters.",
            ));
        }
        if catalog
            .statuses
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(name))
        {
            return Err(ApiError::bad_request(format!(
                "A status named {name} already exists."
            )));
        }
        let category = category_id(input.status_category.as_deref().unwrap_or("TODO"))?;
        let status = meta::create_status(&state.db, name, &input.description, category).await?;
        created.push(json!({ "id": status.id.to_string(), "name": status.name }));
    }
    Ok(Json(json!(created)))
}

/// `PUT /statuses`: renames statuses or changes their categories.
pub async fn update_statuses(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<StatusesRequest>,
) -> ApiResult<StatusCode> {
    let catalog = Catalog::load(&state.db).await?;
    for input in &request.statuses {
        let id = input
            .id
            .as_ref()
            .and_then(json_id)
            .ok_or_else(|| ApiError::bad_request("A status id is required."))?;
        let status = catalog
            .status(id)
            .ok_or_else(|| ApiError::not_found(format!("Status {id} does not exist.")))?;
        let name = input.name.as_deref().map(str::trim).unwrap_or(&status.name);
        let category = match &input.status_category {
            Some(c) => category_id(c)?,
            None => status.category_id,
        };
        meta::update_status(&state.db, id, name, &input.description, category).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct IdsQuery {
    id: Option<String>,
}

/// `DELETE /statuses?id=1,2`: deletes statuses no workflow or issue uses.
pub async fn delete_statuses(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(query): Query<IdsQuery>,
) -> ApiResult<StatusCode> {
    let ids: Vec<i64> = query
        .id
        .unwrap_or_default()
        .split(',')
        .filter_map(|i| i.trim().parse().ok())
        .collect();
    if ids.is_empty() {
        return Err(ApiError::bad_request(
            "Give the ids of the statuses to delete.",
        ));
    }
    for id in &ids {
        if meta::status_in_use(&state.db, *id).await? {
            return Err(ApiError::bad_request(format!(
                "Status {id} is used by a workflow or an issue."
            )));
        }
    }
    for id in ids {
        meta::delete_status(&state.db, id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSearch {
    search_string: Option<String>,
    status_category: Option<String>,
}

/// `GET /statuses/search`.
pub async fn search_statuses(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<StatusSearch>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let needle = query.search_string.unwrap_or_default().to_lowercase();
    let category = match &query.status_category {
        Some(c) => Some(category_id(c)?),
        None => None,
    };
    let matching: Vec<Value> = ctx
        .catalog
        .statuses
        .iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&needle) && category.is_none_or(|c| c == s.category_id)
        })
        .map(status_json)
        .collect();
    let (start_at, max_results) = page.bounded(200);
    let total = matching.len() as i64;
    Ok(Json(json!({
        "self": ctx.api_url("/statuses/search"),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": matching.into_iter().skip(start_at as usize).take(max_results as usize).collect::<Vec<_>>(),
    })))
}

// Workflows.

/// A workflow with its statuses and transitions. Rows of one transition that start from
/// different statuses are shown together, as one transition with several `from` statuses.
async fn workflow_json(
    state: &AppState,
    catalog: &Catalog,
    workflow: &Workflow,
) -> ApiResult<Value> {
    let status_ids = workflows::statuses(&state.db, workflow.id).await?;
    let initial = workflows::initial_status(&state.db, workflow.id).await?;
    let statuses: Vec<Value> = status_ids
        .iter()
        .filter_map(|id| catalog.status(*id))
        .map(|s| json!({ "id": s.id.to_string(), "name": s.name }))
        .collect();
    let mut grouped: BTreeMap<(String, i64), (i64, Vec<String>)> = BTreeMap::new();
    for t in workflows::transitions(&state.db, workflow.id).await? {
        let entry = grouped
            .entry((t.name.clone(), t.to_status_id))
            .or_insert((t.id, Vec::new()));
        if let Some(from) = t.from_status_id {
            entry.1.push(from.to_string());
        }
    }
    let mut transitions = vec![json!({
        "id": "1",
        "name": "Create",
        "description": "",
        "from": [],
        "to": initial.to_string(),
        "type": "initial",
    })];
    for ((name, to), (id, from)) in grouped {
        transitions.push(json!({
            "id": id.to_string(),
            "name": name,
            "description": "",
            "type": if from.is_empty() { "global" } else { "directed" },
            "from": from,
            "to": to.to_string(),
        }));
    }
    Ok(json!({
        "id": { "name": workflow.name, "entityId": workflow.id.to_string() },
        "description": workflow.description,
        "statuses": statuses,
        "transitions": transitions,
        "isDefault": workflow.id == 1,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSearch {
    workflow_name: Option<String>,
}

/// `GET /workflow/search`.
pub async fn search_workflows(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<WorkflowSearch>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let names: Vec<String> = query
        .workflow_name
        .map(|n| n.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let all: Vec<Workflow> = workflows::all(&state.db)
        .await?
        .into_iter()
        .filter(|w| names.is_empty() || names.contains(&w.name))
        .collect();
    let (start_at, max_results) = page.bounded(50);
    let mut values = Vec::new();
    for workflow in all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
    {
        values.push(workflow_json(&state, &ctx.catalog, workflow).await?);
    }
    let total = all.len() as i64;
    Ok(Json(json!({
        "self": ctx.api_url("/workflow/search"),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

#[derive(Deserialize)]
pub struct TransitionInput {
    name: String,
    #[serde(default)]
    from: Vec<Value>,
    to: Value,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
pub struct WorkflowRequest {
    name: String,
    #[serde(default)]
    description: String,
    statuses: Vec<Value>,
    transitions: Vec<TransitionInput>,
}

/// `POST /workflow`: creates a workflow. One `initial` transition names the first status;
/// `global` transitions lead to their status from anywhere; `directed` ones from the listed
/// statuses.
pub async fn create_workflow(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<WorkflowRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("The workflow needs a name."));
    }
    if workflows::by_name(&state.db, name).await?.is_some() {
        return Err(ApiError::bad_request(format!(
            "A workflow named {name} already exists."
        )));
    }
    let catalog = Catalog::load(&state.db).await?;
    let status_id = |value: &Value| -> ApiResult<i64> {
        let id = json_id(value)
            .or_else(|| value.get("id").and_then(json_id))
            .ok_or_else(|| ApiError::bad_request("Statuses are given by id."))?;
        catalog
            .status(id)
            .map(|s| s.id)
            .ok_or_else(|| ApiError::bad_request(format!("Status {id} does not exist.")))
    };
    let statuses: Vec<i64> = request
        .statuses
        .iter()
        .map(status_id)
        .collect::<ApiResult<_>>()?;
    let known: HashSet<i64> = statuses.iter().copied().collect();
    if known.is_empty() {
        return Err(ApiError::bad_request(
            "A workflow needs at least one status.",
        ));
    }
    let mut initial = None;
    let mut parsed: Vec<(String, Vec<i64>, i64)> = Vec::new();
    for transition in &request.transitions {
        let to = status_id(&transition.to)?;
        if !known.contains(&to) {
            return Err(ApiError::bad_request(format!(
                "The transition {} leads to a status outside the workflow.",
                transition.name
            )));
        }
        let from: Vec<i64> = transition
            .from
            .iter()
            .map(status_id)
            .collect::<ApiResult<_>>()?;
        if from.iter().any(|f| !known.contains(f)) {
            return Err(ApiError::bad_request(format!(
                "The transition {} starts outside the workflow.",
                transition.name
            )));
        }
        match transition.kind.as_str() {
            "initial" => initial = Some(to),
            "global" => parsed.push((transition.name.clone(), Vec::new(), to)),
            "directed" if !from.is_empty() => parsed.push((transition.name.clone(), from, to)),
            "directed" => {
                return Err(ApiError::bad_request(
                    "A directed transition needs statuses to start from.",
                ));
            }
            other => {
                return Err(ApiError::bad_request(format!(
                    "Unknown transition type: {other}"
                )));
            }
        }
    }
    let initial =
        initial.ok_or_else(|| ApiError::bad_request("A workflow needs one initial transition."))?;
    let transitions: Vec<NewTransition> = parsed
        .iter()
        .map(|(name, from, to)| NewTransition {
            name,
            from,
            to: *to,
        })
        .collect();
    let workflow = workflows::create(
        &state.db,
        name,
        &request.description,
        &statuses,
        initial,
        &transitions,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "name": workflow.name, "entityId": workflow.id.to_string() })),
    ))
}

/// `DELETE /workflow/{entityId}`: deletes a workflow no scheme uses.
pub async fn delete_workflow(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let workflow = find_workflow(&state, &id).await?;
    if workflow.id == 1 {
        return Err(ApiError::bad_request(
            "The default workflow cannot be deleted.",
        ));
    }
    if workflows::in_use(&state.db, workflow.id).await? {
        return Err(ApiError::bad_request(
            "The workflow is used by a workflow scheme.",
        ));
    }
    workflows::delete(&state.db, workflow.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn find_workflow(state: &AppState, id_or_name: &str) -> ApiResult<Workflow> {
    let found = match id_or_name.parse::<i64>() {
        Ok(id) => workflows::by_id(&state.db, id).await?,
        Err(_) => workflows::by_name(&state.db, id_or_name).await?,
    };
    found.ok_or_else(|| ApiError::not_found(format!("Workflow {id_or_name} does not exist.")))
}

// Workflow schemes.

async fn scheme_json(ctx: &Ctx, state: &AppState, scheme: &Scheme) -> ApiResult<Value> {
    let name_of = |id: i64| async move {
        workflows::by_id(&state.db, id)
            .await
            .map(|w| w.map(|w| w.name).unwrap_or_default())
    };
    let mut mappings = Map::new();
    for (issue_type, workflow) in workflows::mappings(&state.db, scheme.id).await? {
        mappings.insert(issue_type.to_string(), json!(name_of(workflow).await?));
    }
    Ok(json!({
        "id": scheme.id,
        "name": scheme.name,
        "description": scheme.description,
        "defaultWorkflow": name_of(scheme.default_workflow_id).await?,
        "issueTypeMappings": mappings,
        "self": ctx.api_url(&format!("/workflowscheme/{}", scheme.id)),
    }))
}

async fn find_scheme(state: &AppState, id: i64) -> ApiResult<Scheme> {
    workflows::scheme(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Workflow scheme {id} does not exist.")))
}

pub async fn schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = workflows::schemes(&state.db).await?;
    let (start_at, max_results) = page.bounded(50);
    let mut values = Vec::new();
    for scheme in all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
    {
        values.push(scheme_json(&ctx, &state, scheme).await?);
    }
    let total = all.len() as i64;
    Ok(Json(json!({
        "self": ctx.api_url("/workflowscheme"),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

pub async fn scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(
        scheme_json(&ctx, &state, &find_scheme(&state, id).await?).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeRequest {
    name: Option<String>,
    description: Option<String>,
    default_workflow: Option<String>,
    #[serde(default)]
    issue_type_mappings: Map<String, Value>,
}

/// Reads a scheme request into a default workflow id and (issue type, workflow) mappings.
async fn scheme_parts(
    state: &AppState,
    request: &SchemeRequest,
    current_default: Option<i64>,
) -> ApiResult<(i64, Vec<(i64, i64)>)> {
    let default = match &request.default_workflow {
        Some(name) => find_workflow(state, name).await?.id,
        None => current_default.unwrap_or(1),
    };
    let catalog = Catalog::load(&state.db).await?;
    let mut mappings = Vec::new();
    for (issue_type, workflow) in &request.issue_type_mappings {
        let type_id: i64 = issue_type
            .parse()
            .map_err(|_| ApiError::bad_request(format!("Unknown issue type {issue_type}")))?;
        if catalog.issue_type(type_id).is_none() {
            return Err(ApiError::bad_request(format!(
                "Unknown issue type {issue_type}"
            )));
        }
        let name = workflow
            .as_str()
            .ok_or_else(|| ApiError::bad_request("Mappings name workflows."))?;
        mappings.push((type_id, find_workflow(state, name).await?.id));
    }
    Ok((default, mappings))
}

pub async fn create_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request.name.clone().unwrap_or_default();
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("The workflow scheme needs a name."));
    }
    if workflows::schemes(&state.db)
        .await?
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(name.trim()))
    {
        return Err(ApiError::bad_request(format!(
            "A workflow scheme named {name} already exists."
        )));
    }
    let (default, mappings) = scheme_parts(&state, &request, None).await?;
    let scheme = workflows::save_scheme(
        &state.db,
        None,
        name.trim(),
        request.description.as_deref().unwrap_or(""),
        default,
        &mappings,
    )
    .await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(scheme_json(&ctx, &state, &scheme).await?),
    ))
}

/// `PUT /workflowscheme/{id}`. Projects that use the scheme move their issues to statuses of
/// the new workflows.
pub async fn update_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<Json<Value>> {
    let current = find_scheme(&state, id).await?;
    let (default, mappings) =
        scheme_parts(&state, &request, Some(current.default_workflow_id)).await?;
    let name = request.name.clone().unwrap_or(current.name);
    let description = request.description.clone().unwrap_or(current.description);
    let scheme = workflows::save_scheme(
        &state.db,
        Some(id),
        name.trim(),
        &description,
        default,
        &mappings,
    )
    .await?;
    for project in projects::list(&state.db).await? {
        if workflows::project_scheme(&state.db, project.id).await? == id {
            fit_project(&state, project.id).await?;
        }
    }
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(scheme_json(&ctx, &state, &scheme).await?))
}

pub async fn delete_scheme(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_scheme(&state, id).await?;
    if id == workflows::DEFAULT_SCHEME {
        return Err(ApiError::bad_request(
            "The default workflow scheme cannot be deleted.",
        ));
    }
    if workflows::scheme_in_use(&state.db, id).await? {
        return Err(ApiError::bad_request(
            "The workflow scheme is used by a project.",
        ));
    }
    workflows::delete_scheme(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdsQuery {
    project_id: Option<String>,
}

/// `GET /workflowscheme/project?projectId=...`: the scheme of each project.
pub async fn project_schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<ProjectIdsQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let ids: Vec<i64> = query
        .project_id
        .unwrap_or_default()
        .split(',')
        .filter_map(|i| i.trim().parse().ok())
        .collect();
    let mut by_scheme: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for id in ids {
        if projects::by_id(&state.db, id).await?.is_some() {
            by_scheme
                .entry(workflows::project_scheme(&state.db, id).await?)
                .or_default()
                .push(id.to_string());
        }
    }
    let mut values = Vec::new();
    for (scheme_id, project_ids) in by_scheme {
        let scheme = find_scheme(&state, scheme_id).await?;
        values.push(json!({ "projectIds": project_ids, "workflowScheme": scheme_json(&ctx, &state, &scheme).await? }));
    }
    Ok(Json(json!({ "values": values })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignRequest {
    workflow_scheme_id: Value,
    project_id: Value,
}

/// `PUT /workflowscheme/project`: gives a project another scheme.
pub async fn assign_scheme(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<AssignRequest>,
) -> ApiResult<StatusCode> {
    let scheme_id = json_id(&request.workflow_scheme_id)
        .ok_or_else(|| ApiError::bad_request("workflowSchemeId is required."))?;
    let project_id = json_id(&request.project_id)
        .ok_or_else(|| ApiError::bad_request("projectId is required."))?;
    find_scheme(&state, scheme_id).await?;
    projects::by_id(&state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::project_not_found(&project_id.to_string()))?;
    workflows::set_project_scheme(&state.db, project_id, scheme_id).await?;
    fit_project(&state, project_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Makes a project match its workflows after they change. Issues in a status their workflow
/// lacks move to the workflow's first status of the same category, or to its initial status.
/// Boards get a column for each status they do not show yet.
async fn fit_project(state: &AppState, project_id: i64) -> ApiResult<()> {
    let catalog = Catalog::load(&state.db).await?;
    for issue_type in &catalog.issue_types {
        let workflow = workflows::for_issue_type(&state.db, project_id, issue_type.id).await?;
        let allowed = workflows::statuses(&state.db, workflow).await?;
        let initial = workflows::initial_status(&state.db, workflow).await?;
        for status in issues::statuses_in_use(&state.db, project_id, issue_type.id).await? {
            if allowed.contains(&status) {
                continue;
            }
            let category = catalog.status(status).map(|s| s.category_id);
            let target = allowed
                .iter()
                .copied()
                .find(|s| catalog.status(*s).map(|x| x.category_id) == category)
                .unwrap_or(initial);
            issues::move_status(&state.db, project_id, issue_type.id, status, target).await?;
        }
    }
    let statuses = workflows::project_statuses(&state.db, project_id).await?;
    for board in boards::all(&state.db).await? {
        if board.project_id != Some(project_id) {
            continue;
        }
        let shown = boards::mapped_statuses(&state.db, board.id).await?;
        for status in statuses.iter().filter(|s| !shown.contains(s)) {
            let Some(found) = catalog.status(*status) else {
                continue;
            };
            // Done statuses go last; others go before the last column.
            let done = found.category_id == 3;
            boards::add_status_column(&state.db, board.id, &found.name, found.id, !done).await?;
        }
    }
    Ok(())
}
