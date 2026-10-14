//! Project components and versions: `/component`, `/version`, and their project listings.

use super::extract::Json;
use crate::api::access::require_project;
use crate::store::users::User;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::components::{self, Component, ComponentFields};
use crate::store::meta::json_id;
use crate::store::projects::{self, Project};
use crate::store::users;
use crate::store::versions::{self, Version, VersionFields};

const ASSIGNEE_TYPES: &[&str] = &[
    "PROJECT_DEFAULT",
    "COMPONENT_LEAD",
    "PROJECT_LEAD",
    "UNASSIGNED",
];

/// Who issues in a component go to: the assignee type after resolving PROJECT_DEFAULT, and the user.
fn real_assignee(project: &Project, component: &Component) -> (&'static str, Option<i64>) {
    match component.assignee_type.as_str() {
        "COMPONENT_LEAD" => ("COMPONENT_LEAD", component.lead_id),
        "PROJECT_LEAD" => ("PROJECT_LEAD", project.lead_id),
        "UNASSIGNED" => ("UNASSIGNED", None),
        _ if project.assignee_type == "PROJECT_LEAD" => ("PROJECT_LEAD", project.lead_id),
        _ => ("UNASSIGNED", None),
    }
}

/// The automatic assignee for a new issue: the first component that names someone, then the
/// project's default. This is what Jira does when an issue is created without an assignee.
pub async fn default_assignee(
    db: &SqlitePool,
    project: &Project,
    component_ids: &[i64],
) -> ApiResult<Option<i64>> {
    for id in component_ids {
        if let Some(component) = components::by_id(db, *id).await?
            && let (_, Some(user)) = real_assignee(project, &component)
        {
            return Ok(Some(user));
        }
    }
    Ok(match project.assignee_type.as_str() {
        "PROJECT_LEAD" => project.lead_id,
        _ => None,
    })
}

async fn component_json(ctx: &mut Ctx, component: &Component) -> ApiResult<Value> {
    let project = projects::by_id(&ctx.state.db, component.project_id)
        .await?
        .expect("components always have a project");
    let (real_type, real_user) = real_assignee(&project, component);
    let mut value = json!({
        "self": ctx.api_url(&format!("/component/{}", component.id)),
        "id": component.id.to_string(),
        "name": component.name,
        "description": component.description,
        "assigneeType": component.assignee_type,
        "realAssigneeType": real_type,
        "isAssigneeTypeValid": true,
        "project": project.key,
        "projectId": project.id,
    });
    if component.lead_id.is_some() {
        let lead = ctx.user_ref(component.lead_id).await?;
        value["leadAccountId"] = lead["accountId"].clone();
        value["lead"] = lead;
    }
    if real_user.is_some() {
        value["realAssignee"] = ctx.user_ref(real_user).await?;
        if component.assignee_type != "UNASSIGNED" {
            value["assignee"] = value["realAssignee"].clone();
        }
    }
    Ok(value)
}

/// Finds the project of a create request, given as `project` (key) or `projectId`.
async fn request_project(state: &AppState, body: &Value) -> ApiResult<Project> {
    let reference = body
        .get("project")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            body.get("projectId")
                .and_then(json_id)
                .map(|id| id.to_string())
        })
        .ok_or_else(|| ApiError::field("project", "The project is required."))?;
    projects::by_id_or_key(&state.db, &reference)
        .await?
        .ok_or_else(|| {
            ApiError::field(
                "project",
                format!("The project '{reference}' does not exist."),
            )
        })
}

async fn find_project(state: &AppState, id_or_key: &str) -> ApiResult<Project> {
    projects::by_id_or_key(&state.db, id_or_key)
        .await?
        .ok_or_else(|| ApiError::project_not_found(id_or_key))
}

/// Checks a permission in the project with this id.
async fn check_project(
    state: &AppState,
    user: &User,
    project_id: i64,
    permission: &str,
) -> ApiResult<()> {
    let project = projects::by_id(&state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::project_not_found(&project_id.to_string()))?;
    require_project(state, user, &project, permission).await?;
    Ok(())
}

fn paged(ctx: &Ctx, path: &str, values: Vec<Value>, page: &Page) -> Value {
    let (start_at, max_results) = page.bounded(1000);
    let total = values.len() as i64;
    let values: Vec<Value> = values
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    json!({
        "self": ctx.api_url(&format!("{path}?startAt={start_at}&maxResults={max_results}")),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })
}

// ----- Components -----

/// The editable fields of a component, from a create or update request.
async fn component_fields(
    state: &AppState,
    body: &Value,
    current: Option<&Component>,
) -> ApiResult<ComponentFields> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    let name = text("name")
        .or_else(|| current.map(|c| c.name.clone()))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::field("name", "The component name must not be empty."))?;
    if name.chars().count() > 255 {
        return Err(ApiError::field(
            "name",
            "The component name must be 255 characters or less.",
        ));
    }
    let assignee_type = text("assigneeType")
        .or_else(|| current.map(|c| c.assignee_type.clone()))
        .unwrap_or_else(|| "PROJECT_DEFAULT".into());
    if !ASSIGNEE_TYPES.contains(&assignee_type.as_str()) {
        return Err(ApiError::field(
            "assigneeType",
            "The assignee type is not valid.",
        ));
    }
    let lead_id = match body.get("leadAccountId") {
        Some(Value::Null) => None,
        Some(Value::String(account_id)) => Some(
            users::by_account_id(&state.db, account_id)
                .await?
                .ok_or_else(|| ApiError::field("leadAccountId", "The user does not exist."))?
                .id,
        ),
        _ => current.and_then(|c| c.lead_id),
    };
    Ok(ComponentFields {
        name,
        description: text("description")
            .or_else(|| current.map(|c| c.description.clone()))
            .unwrap_or_default(),
        lead_id,
        assignee_type,
    })
}

async fn find_component(state: &AppState, id: &str) -> ApiResult<Component> {
    let not_found = || ApiError::not_found(format!("The component with id {id} does not exist."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    components::by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)
}

/// Checks that no other component in the project has the same name.
async fn check_component_name(
    state: &AppState,
    project_id: i64,
    name: &str,
    except: Option<i64>,
) -> ApiResult<()> {
    let taken = components::for_project(&state.db, project_id)
        .await?
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name) && Some(c.id) != except);
    if taken {
        return Err(ApiError::field(
            "name",
            format!("A component with the name {name} already exists in this project."),
        ));
    }
    Ok(())
}

pub async fn create_component(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let project = request_project(&state, &body).await?;
    check_project(&state, &user, project.id, "ADMINISTER_PROJECTS").await?;
    let fields = component_fields(&state, &body, None).await?;
    check_component_name(&state, project.id, &fields.name, None).await?;
    let component = components::create(&state.db, project.id, &fields).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(component_json(&mut ctx, &component).await?),
    ))
}

pub async fn get_component(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let component = find_component(&state, &id).await?;
    check_project(&state, &user, component.project_id, "BROWSE_PROJECTS").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(component_json(&mut ctx, &component).await?))
}

pub async fn update_component(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let component = find_component(&state, &id).await?;
    check_project(&state, &user, component.project_id, "ADMINISTER_PROJECTS").await?;
    let fields = component_fields(&state, &body, Some(&component)).await?;
    check_component_name(
        &state,
        component.project_id,
        &fields.name,
        Some(component.id),
    )
    .await?;
    components::update(&state.db, component.id, &fields).await?;
    let updated = find_component(&state, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(component_json(&mut ctx, &updated).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteComponentQuery {
    move_issues_to: Option<i64>,
}

pub async fn delete_component(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<DeleteComponentQuery>,
) -> ApiResult<StatusCode> {
    let component = find_component(&state, &id).await?;
    check_project(&state, &user, component.project_id, "ADMINISTER_PROJECTS").await?;
    if let Some(target) = query.move_issues_to {
        let valid = components::by_id(&state.db, target)
            .await?
            .is_some_and(|t| t.project_id == component.project_id && t.id != component.id);
        if !valid {
            return Err(ApiError::bad_request(format!(
                "The component with id {target} does not exist in this project."
            )));
        }
    }
    components::delete(&state.db, component.id, query.move_issues_to).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn component_issue_count(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let component = find_component(&state, &id).await?;
    check_project(&state, &user, component.project_id, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/component/{}/relatedIssueCounts", component.id)),
        "issueCount": components::issue_count(&state.db, component.id).await?,
    })))
}

async fn project_components_json(ctx: &mut Ctx, project: &Project) -> ApiResult<Vec<Value>> {
    let mut values = Vec::new();
    for component in components::for_project(&ctx.state.db, project.id).await? {
        values.push(component_json(ctx, &component).await?);
    }
    Ok(values)
}

pub async fn project_components(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let project = find_project(&state, &id_or_key).await?;
    check_project(&state, &user, project.id, "BROWSE_PROJECTS").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(project_components_json(&mut ctx, &project).await?))
}

pub async fn project_components_paged(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &id_or_key).await?;
    check_project(&state, &user, project.id, "BROWSE_PROJECTS").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let values = project_components_json(&mut ctx, &project).await?;
    Ok(Json(paged(
        &ctx,
        &format!("/project/{}/component", project.key),
        values,
        &page,
    )))
}

// ----- Versions -----

/// Dates are `yyyy-MM-dd`. `null` clears the date.
fn version_date(body: &Value, key: &str, current: Option<String>) -> ApiResult<Option<String>> {
    match body.get(key) {
        None => Ok(current),
        Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if crate::time::parse_date(text).is_some() => {
            Ok(Some(text.clone()))
        }
        Some(_) => Err(ApiError::field(
            key,
            "The date must be in the format yyyy-MM-dd.",
        )),
    }
}

fn version_fields(body: &Value, current: Option<&Version>) -> ApiResult<VersionFields> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    let flag = |key: &str, now: Option<bool>| {
        body.get(key)
            .and_then(Value::as_bool)
            .or(now)
            .unwrap_or(false)
    };
    let name = text("name")
        .or_else(|| current.map(|v| v.name.clone()))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::field("name", "You must specify a valid version name"))?;
    if name.chars().count() > 255 {
        return Err(ApiError::field(
            "name",
            "The version name must be 255 characters or less.",
        ));
    }
    let start_date = version_date(
        body,
        "startDate",
        current.and_then(|v| v.start_date.clone()),
    )?;
    let release_date = version_date(
        body,
        "releaseDate",
        current.and_then(|v| v.release_date.clone()),
    )?;
    if let (Some(start), Some(release)) = (&start_date, &release_date)
        && start > release
    {
        return Err(ApiError::field(
            "startDate",
            "The start date must be before the release date.",
        ));
    }
    Ok(VersionFields {
        name,
        description: text("description")
            .or_else(|| current.map(|v| v.description.clone()))
            .unwrap_or_default(),
        archived: flag("archived", current.map(|v| v.archived)),
        released: flag("released", current.map(|v| v.released)),
        start_date,
        release_date,
    })
}

async fn find_version(state: &AppState, id: &str) -> ApiResult<Version> {
    let not_found = || ApiError::not_found(format!("Could not find version for id '{id}'"));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    versions::by_id(&state.db, id).await?.ok_or_else(not_found)
}

async fn check_version_name(
    state: &AppState,
    project_id: i64,
    name: &str,
    except: Option<i64>,
) -> ApiResult<()> {
    let taken = versions::for_project(&state.db, project_id)
        .await?
        .iter()
        .any(|v| v.name.eq_ignore_ascii_case(name) && Some(v.id) != except);
    if taken {
        return Err(ApiError::field(
            "name",
            "A version with this name already exists in this project.",
        ));
    }
    Ok(())
}

pub async fn create_version(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let project = request_project(&state, &body).await?;
    check_project(&state, &user, project.id, "ADMINISTER_PROJECTS").await?;
    let fields = version_fields(&body, None)?;
    check_version_name(&state, project.id, &fields.name, None).await?;
    let created = versions::create(&state.db, project.id, &fields).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok((StatusCode::CREATED, Json(ctx.version_json(&created))))
}

pub async fn get_version(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.version_json(&found)))
}

pub async fn update_version(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "ADMINISTER_PROJECTS").await?;
    let fields = version_fields(&body, Some(&found))?;
    check_version_name(&state, found.project_id, &fields.name, Some(found.id)).await?;
    versions::update(&state.db, found.id, &fields).await?;
    let updated = find_version(&state, &id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.version_json(&updated)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteVersionQuery {
    move_fix_issues_to: Option<i64>,
    move_affected_issues_to: Option<i64>,
}

pub async fn delete_version(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<DeleteVersionQuery>,
) -> ApiResult<StatusCode> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "ADMINISTER_PROJECTS").await?;
    for target in [query.move_fix_issues_to, query.move_affected_issues_to]
        .into_iter()
        .flatten()
    {
        let valid = versions::by_id(&state.db, target)
            .await?
            .is_some_and(|t| t.project_id == found.project_id && t.id != found.id);
        if !valid {
            return Err(ApiError::bad_request(format!(
                "The version with id {target} does not exist in this project."
            )));
        }
    }
    versions::delete(
        &state.db,
        found.id,
        query.move_fix_issues_to,
        query.move_affected_issues_to,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /version/{id}/move` with `{"position": "First" | "Last" | "Earlier" | "Later"}`, or
/// `{"after": "<version url>"}`.
pub async fn move_version(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "ADMINISTER_PROJECTS").await?;
    let ordered = versions::for_project(&state.db, found.project_id).await?;
    let current = ordered.iter().position(|v| v.id == found.id).unwrap_or(0) as i64 + 1;
    let position = if let Some(after) = body.get("after").and_then(Value::as_str) {
        let after_id: i64 = after
            .rsplit('/')
            .next()
            .and_then(|id| id.parse().ok())
            .ok_or_else(|| ApiError::field("after", "The version URL is not valid."))?;
        let index = ordered
            .iter()
            .filter(|v| v.id != found.id)
            .position(|v| v.id == after_id)
            .ok_or_else(|| {
                ApiError::field("after", "The version does not exist in this project.")
            })?;
        index as i64 + 2
    } else {
        match body.get("position").and_then(Value::as_str) {
            Some("First") => 1,
            Some("Last") => ordered.len() as i64,
            Some("Earlier") => current - 1,
            Some("Later") => current + 1,
            _ => {
                return Err(ApiError::field(
                    "position",
                    "The position must be First, Last, Earlier, or Later.",
                ));
            }
        }
    };
    versions::move_to(&state.db, &found, position).await?;
    let moved = find_version(&state, &id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.version_json(&moved)))
}

pub async fn version_issue_counts(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "BROWSE_PROJECTS").await?;
    let (fixed, affected) = versions::issue_counts(&state.db, found.id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/version/{}", found.id)),
        "issuesFixedCount": fixed,
        "issuesAffectedCount": affected,
        "issueCountWithCustomFieldsShowingVersion": 0,
    })))
}

pub async fn version_unresolved_count(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let found = find_version(&state, &id).await?;
    check_project(&state, &user, found.project_id, "BROWSE_PROJECTS").await?;
    let (fixed, _) = versions::issue_counts(&state.db, found.id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/version/{}", found.id)),
        "issuesUnresolvedCount": versions::unresolved_count(&state.db, found.id).await?,
        "issuesCount": fixed,
    })))
}

pub async fn project_versions(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let project = find_project(&state, &id_or_key).await?;
    check_project(&state, &user, project.id, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    let all = versions::for_project(&state.db, project.id).await?;
    Ok(Json(all.iter().map(|v| ctx.version_json(v)).collect()))
}

pub async fn project_versions_paged(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &id_or_key).await?;
    check_project(&state, &user, project.id, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    let values = versions::for_project(&state.db, project.id)
        .await?
        .iter()
        .map(|v| ctx.version_json(v))
        .collect();
    Ok(Json(paged(
        &ctx,
        &format!("/project/{}/version", project.key),
        values,
        &page,
    )))
}

/// Components and versions of an issue, rendered for its `components`, `fixVersions`, and
/// `versions` fields.
pub async fn issue_parts_json(ctx: &mut Ctx, issue_id: i64) -> ApiResult<(Value, Value, Value)> {
    let db = ctx.state.db.clone();
    let mut rendered_components = Vec::new();
    for id in components::of_issue(&db, issue_id).await? {
        if let Some(component) = components::by_id(&db, id).await? {
            rendered_components.push(json!({
                "self": ctx.api_url(&format!("/component/{}", component.id)),
                "id": component.id.to_string(),
                "name": component.name,
                "description": component.description,
            }));
        }
    }
    let mut lists = Vec::new();
    for kind in [versions::Kind::Fix, versions::Kind::Affects] {
        let mut rendered = Vec::new();
        for id in versions::of_issue(&db, issue_id, kind).await? {
            if let Some(version) = versions::by_id(&db, id).await? {
                rendered.push(ctx.version_json(&version));
            }
        }
        lists.push(Value::Array(rendered));
    }
    let affects = lists.pop().expect("two lists");
    let fix = lists.pop().expect("two lists");
    Ok((Value::Array(rendered_components), fix, affects))
}
