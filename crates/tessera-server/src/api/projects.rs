//! Projects: `/project`, `/project/search`, `/project/{idOrKey}`, `/project/{idOrKey}/statuses`.

use super::extract::Json;
use crate::api::access::{browsable_projects, require_project};
use crate::store::permissions;
use crate::store::users::User;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::filters::{self, Share};
use crate::store::projects::{self, NewProject, Project, ProjectUpdate};
use crate::store::{security, users, workflows};

/// The full project object, as returned by `GET /project/{idOrKey}`.
async fn full_project(ctx: &mut Ctx, project: &Project) -> ApiResult<Value> {
    let mut value = ctx.project_ref(project);
    value["description"] = json!(project.description);
    value["lead"] = ctx.user_ref(project.lead_id).await?;
    value["issueTypes"] = json!(
        ctx.catalog
            .issue_types
            .iter()
            .map(|t| ctx.issue_type(t))
            .collect::<Vec<_>>()
    );
    value["assigneeType"] = json!(project.assignee_type);
    value["url"] = json!(project.url);
    if let Some(category) = match project.category_id {
        Some(id) => projects::category(&ctx.state.db, id).await?,
        None => None,
    } {
        value["projectCategory"] = super::schemes::category_json(ctx, &category);
    }
    value["components"] = json!([]);
    value["versions"] = json!([]);
    value["isPrivate"] = json!(false);
    value["style"] = json!("classic");
    Ok(value)
}

async fn find(state: &AppState, id_or_key: &str) -> ApiResult<Project> {
    projects::by_id_or_key(&state.db, id_or_key)
        .await?
        .ok_or_else(|| ApiError::project_not_found(id_or_key))
}

/// A project the user can browse. Others are reported as missing, as in Jira.
async fn find_visible(state: &AppState, user: &User, id_or_key: &str) -> ApiResult<Project> {
    let project = find(state, id_or_key).await?;
    require_project(state, user, &project, "BROWSE_PROJECTS").await?;
    Ok(project)
}

/// The projects a user can browse.
async fn visible_projects(state: &AppState, user: &User) -> ApiResult<Vec<Project>> {
    let all = projects::list(&state.db).await?;
    Ok(match browsable_projects(&state.db, user).await? {
        None => all,
        Some(ids) => all.into_iter().filter(|p| ids.contains(&p.id)).collect(),
    })
}

/// All projects as a plain array.
pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = visible_projects(&state, &user).await?;
    Ok(Json(all.iter().map(|p| ctx.project_ref(p)).collect()))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    query: Option<String>,
}

/// A page of projects whose key or name contains `query`.
pub async fn search(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let needle = query.query.unwrap_or_default().to_lowercase();
    let matching: Vec<Project> = visible_projects(&state, &user)
        .await?
        .into_iter()
        .filter(|p| {
            p.key.to_lowercase().contains(&needle) || p.name.to_lowercase().contains(&needle)
        })
        .collect();
    let (start_at, max_results) = page.bounded(100);
    let total = matching.len() as i64;
    let values: Vec<Value> = matching
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .map(|p| ctx.project_ref(p))
        .collect();
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/project/search?startAt={start_at}&maxResults={max_results}")),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = find_visible(&state, &user, &id_or_key).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(full_project(&mut ctx, &project).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProjectRequest {
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    project_type_key: Option<String>,
    lead_account_id: Option<String>,
    /// Jira Server sends the lead as `lead`, a username. Tessera matches it by email.
    lead: Option<String>,
    category_id: Option<i64>,
    #[serde(default)]
    url: String,
    assignee_type: Option<String>,
    /// Software templates (scrum, kanban, basic) also create a board, as in Jira.
    project_template_key: Option<String>,
    /// The issue security scheme, by id (a number or a string).
    issue_security_scheme: Option<Value>,
    // `permissionScheme` and `notificationScheme` are accepted and ignored: new projects use
    // the default permission scheme, and Tessera has one notification setup.
}

fn check_assignee_type(value: Option<&str>) -> ApiResult<()> {
    match value {
        None | Some("PROJECT_LEAD" | "UNASSIGNED") => Ok(()),
        Some(_) => Err(ApiError::field(
            "assigneeType",
            "The default assignee must be PROJECT_LEAD or UNASSIGNED.",
        )),
    }
}

async fn check_category(state: &AppState, id: Option<i64>) -> ApiResult<()> {
    match id {
        Some(id) if projects::category(&state.db, id).await?.is_none() => Err(ApiError::field(
            "categoryId",
            format!("The project category with id {id} does not exist."),
        )),
        _ => Ok(()),
    }
}

/// Resolves a project lead by account id, or by email for Server-style clients.
async fn lead_id(
    state: &AppState,
    account_id: Option<&str>,
    email: Option<&str>,
) -> ApiResult<Option<i64>> {
    let not_found = || {
        ApiError::field(
            "projectLead",
            "The project lead you specified does not exist.",
        )
    };
    let user = match (account_id, email) {
        (Some(account_id), _) => users::by_account_id(&state.db, account_id).await?,
        (None, Some(email)) => users::by_email(&state.db, email).await?,
        (None, None) => return Ok(None),
    };
    Ok(Some(user.ok_or_else(not_found)?.id))
}

/// Creates a project. Site administrators only.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(request): Json<NewProjectRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let key = request.key.trim();
    if !projects::is_valid_key(key) {
        return Err(ApiError::field(
            "projectKey",
            "Project keys must start with an uppercase letter, followed by one or more uppercase alphanumeric characters.",
        ));
    }
    if projects::by_id_or_key(&state.db, key).await?.is_some() {
        return Err(ApiError::field(
            "projectKey",
            format!("Project '{key}' uses this project key."),
        ));
    }
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::field(
            "projectName",
            "You must specify a valid project name.",
        ));
    }
    let all = projects::list(&state.db).await?;
    if all.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Err(ApiError::field(
            "projectName",
            "A project with that name already exists.",
        ));
    }
    let project_type_key = request.project_type_key.as_deref().unwrap_or("software");
    if !matches!(project_type_key, "software" | "business" | "service_desk") {
        return Err(ApiError::field(
            "projectTypeKey",
            "The project type is not valid.",
        ));
    }
    check_assignee_type(request.assignee_type.as_deref())?;
    check_category(&state, request.category_id).await?;
    let security_scheme = match &request.issue_security_scheme {
        None | Some(Value::Null) => None,
        Some(value) => {
            let id = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
            let found = match id {
                Some(id) => security::scheme(&state.db, id).await?,
                None => None,
            };
            Some(found.ok_or_else(|| {
                ApiError::field(
                    "issueSecurityScheme",
                    "The issue security scheme does not exist.",
                )
            })?)
        }
    };
    let lead_id = lead_id(
        &state,
        request.lead_account_id.as_deref(),
        request.lead.as_deref(),
    )
    .await?
    .or(Some(admin.id));
    let project = projects::create(
        &state.db,
        NewProject {
            key,
            name,
            description: &request.description,
            lead_id,
            category_id: request.category_id,
            url: &request.url,
            assignee_type: request.assignee_type.as_deref().unwrap_or("UNASSIGNED"),
            project_type_key,
        },
    )
    .await?;
    // The lead (or the creator, without one) administers the new project.
    let administrator = lead_id.unwrap_or(admin.id);
    permissions::add_role_user(
        &state.db,
        project.id,
        permissions::ADMINISTRATORS,
        administrator,
    )
    .await?;
    if let Some(scheme) = security_scheme {
        security::set_project_scheme(&state.db, project.id, Some(scheme.id), &[]).await?;
    }
    if let Some(board_type) = template_board_type(&project, request.project_template_key.as_deref())
    {
        create_project_board(&state, admin.id, &project, board_type).await?;
    }
    // Jira returns a numeric id here, unlike everywhere else.
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "self": format!("{}/rest/api/3/project/{}", state.base_url, project.id),
            "id": project.id,
            "key": project.key,
        })),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdateRequest {
    name: Option<String>,
    description: Option<String>,
    lead_account_id: Option<String>,
    lead: Option<String>,
    category_id: Option<i64>,
    url: Option<String>,
    assignee_type: Option<String>,
}

/// Updates a project. Needs the Administer Projects permission.
pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<ProjectUpdateRequest>,
) -> ApiResult<Json<Value>> {
    let project = find(&state, &id_or_key).await?;
    require_project(&state, &user, &project, "ADMINISTER_PROJECTS").await?;
    check_assignee_type(request.assignee_type.as_deref())?;
    check_category(&state, request.category_id).await?;
    let lead_id = lead_id(
        &state,
        request.lead_account_id.as_deref(),
        request.lead.as_deref(),
    )
    .await?;
    projects::update(
        &state.db,
        project.id,
        ProjectUpdate {
            name: request.name,
            description: request.description,
            lead_id,
            category_id: request.category_id,
            url: request.url,
            assignee_type: request.assignee_type,
        },
    )
    .await?;
    let updated = find(&state, &project.id.to_string()).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(full_project(&mut ctx, &updated).await?))
}

/// Deletes a project and all of its issues. Site administrators only.
pub async fn delete(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<StatusCode> {
    let project = find(&state, &id_or_key).await?;
    projects::delete(&state.db, project.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// For each issue type, the statuses an issue of that type can have in this project.
pub async fn statuses(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let project = find_visible(&state, &user, &id_or_key).await?;
    let ctx = Ctx::new(&state, version).await?;
    let mut out = Vec::with_capacity(ctx.catalog.issue_types.len());
    // Each issue type follows its own workflow from the project's workflow scheme.
    for issue_type in &ctx.catalog.issue_types {
        let workflow = workflows::for_issue_type(&state.db, project.id, issue_type.id).await?;
        let statuses: Vec<Value> = workflows::statuses(&state.db, workflow)
            .await?
            .iter()
            .filter_map(|id| ctx.catalog.status(*id))
            .map(|s| ctx.status(s))
            .collect();
        out.push(json!({
            "self": ctx.api_url(&format!("/issuetype/{}", issue_type.id)),
            "id": issue_type.id.to_string(),
            "name": issue_type.name,
            "subtask": issue_type.is_subtask(),
            "statuses": statuses,
        }));
    }
    Ok(Json(out))
}

/// The board a project template comes with: scrum for scrum templates, kanban for kanban and
/// basic software templates, none otherwise.
fn template_board_type(project: &Project, template: Option<&str>) -> Option<&'static str> {
    let template = template?.to_ascii_lowercase();
    if project.project_type_key != "software" {
        return None;
    }
    if template.contains("scrum") {
        Some("scrum")
    } else if template.contains("kanban") || template.contains("basic") {
        Some("kanban")
    } else {
        None
    }
}

/// Creates "Filter for KEY board" (shared with the project) and the "KEY board" on it.
async fn create_project_board(
    state: &AppState,
    owner_id: i64,
    project: &Project,
    board_type: &str,
) -> ApiResult<()> {
    let filter = filters::create(
        &state.db,
        owner_id,
        &format!("Filter for {} board", project.key),
        "",
        &format!("project = {} ORDER BY Rank ASC", project.key),
    )
    .await?;
    filters::set_shares(
        &state.db,
        filter.id,
        &[Share {
            share_type: "project".into(),
            project_id: Some(project.id),
        }],
    )
    .await?;
    super::agile::create_board(
        &state.db,
        &format!("{} board", project.key),
        board_type,
        filter.id,
        Some(project),
    )
    .await?;
    Ok(())
}
