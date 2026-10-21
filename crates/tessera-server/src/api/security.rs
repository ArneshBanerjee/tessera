//! Issue security schemes and levels: `/issuesecurityschemes`, `/securitylevel/{id}`, and a
//! project's scheme and settable levels.
//!
//! Site administrators manage schemes. Members of a level are named with the holder types of
//! permission grants (`group`, `user`, `projectRole`, `reporter`, and so on); Jira's lower-case
//! spellings (`projectrole`, `lead`) are accepted too.

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::access::{Access, require_project};
use super::extract::Json;
use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::projects::{self, Project};
use crate::store::security::{self, Level, Member, Scheme};
use crate::store::users::{self, User};
use crate::store::{groups, permissions};

pub fn security_level_json(ctx: &Ctx, level: &Level, is_default: Option<bool>) -> Value {
    let mut out = json!({
        "self": ctx.api_url(&format!("/securitylevel/{}", level.id)),
        "id": level.id.to_string(),
        "description": level.description,
        "name": level.name,
    });
    if let Some(is_default) = is_default {
        out["isDefault"] = json!(is_default);
        out["issueSecuritySchemeId"] = json!(level.scheme_id.to_string());
    }
    out
}

async fn scheme_json(ctx: &Ctx, scheme: &Scheme, with_levels: bool) -> ApiResult<Value> {
    let mut out = json!({
        "self": ctx.api_url(&format!("/issuesecurityschemes/{}", scheme.id)),
        "id": scheme.id,
        "name": scheme.name,
        "description": scheme.description,
    });
    if let Some(level) = scheme.default_level_id {
        out["defaultSecurityLevelId"] = json!(level);
    }
    if with_levels {
        let levels: Vec<Value> = security::levels(&ctx.state.db, scheme.id)
            .await?
            .iter()
            .map(|l| security_level_json(ctx, l, None))
            .collect();
        out["levels"] = json!(levels);
    }
    Ok(out)
}

fn scheme_missing(id: i64) -> ApiError {
    ApiError::not_found(format!(
        "The issue security scheme with id {id} does not exist."
    ))
}

async fn find_scheme(state: &AppState, id: i64) -> ApiResult<Scheme> {
    security::scheme(&state.db, id)
        .await?
        .ok_or_else(|| scheme_missing(id))
}

async fn find_level(state: &AppState, scheme: &Scheme, id: i64) -> ApiResult<Level> {
    security::level(&state.db, id)
        .await?
        .filter(|l| l.scheme_id == scheme.id)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "The issue security level with id {id} does not exist in the scheme."
            ))
        })
}

/// `GET /issuesecurityschemes`.
pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let mut out = Vec::new();
    for scheme in security::schemes(&state.db).await? {
        out.push(scheme_json(&ctx, &scheme, false).await?);
    }
    Ok(Json(json!({ "issueSecuritySchemes": out })))
}

/// `GET /issuesecurityschemes/{id}`, with its levels.
pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let scheme = find_scheme(&state, id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(scheme_json(&ctx, &scheme, true).await?))
}

#[derive(Deserialize)]
pub struct MemberRequest {
    #[serde(rename = "type")]
    holder_type: String,
    parameter: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelRequest {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    members: Vec<MemberRequest>,
}

#[derive(Deserialize)]
pub struct SchemeRequest {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    levels: Vec<LevelRequest>,
}

/// A member's holder type in Tessera's spelling, after checking its parameter.
async fn member_holder(
    state: &AppState,
    member: &MemberRequest,
) -> ApiResult<(&'static str, Option<String>)> {
    let parameter = member.parameter.clone().filter(|p| !p.is_empty());
    let required = || {
        ApiError::bad_request(format!(
            "The member type '{}' needs a parameter.",
            member.holder_type
        ))
    };
    Ok(match member.holder_type.as_str() {
        "reporter" => ("reporter", None),
        "assignee" => ("assignee", None),
        "lead" | "projectLead" => ("projectLead", None),
        "applicationRole" => ("applicationRole", None),
        "anyone" => ("anyone", None),
        "group" => {
            let name = parameter.ok_or_else(required)?;
            let group = groups::find(&state.db, &name).await?.ok_or_else(|| {
                ApiError::bad_request(format!("The group '{name}' does not exist."))
            })?;
            ("group", Some(group.name))
        }
        "user" => {
            let account = parameter.ok_or_else(required)?;
            users::by_account_id(&state.db, &account)
                .await?
                .ok_or_else(|| {
                    ApiError::bad_request(format!("The user '{account}' does not exist."))
                })?;
            ("user", Some(account))
        }
        "projectrole" | "projectRole" => {
            let role = parameter.ok_or_else(required)?;
            let found = match role.parse::<i64>() {
                Ok(id) => permissions::role(&state.db, id).await?.is_some(),
                Err(_) => false,
            };
            if !found {
                return Err(ApiError::bad_request(format!(
                    "The project role '{role}' does not exist."
                )));
            }
            ("projectRole", Some(role))
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "The member type '{other}' is not supported."
            )));
        }
    })
}

/// Adds levels to a scheme, with their members. A level marked `isDefault` becomes the default.
async fn add_levels(state: &AppState, scheme: &Scheme, levels: &[LevelRequest]) -> ApiResult<()> {
    // Check every level and member before writing anything.
    let mut checked = Vec::new();
    for level in levels {
        let name = level
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| ApiError::bad_request("A security level needs a name."))?;
        let mut members = Vec::new();
        for member in &level.members {
            members.push(member_holder(state, member).await?);
        }
        checked.push((name, level, members));
    }
    let mut scheme = scheme.clone();
    for (name, request, members) in checked {
        let level = security::create_level(
            &state.db,
            scheme.id,
            name,
            request.description.as_deref().unwrap_or(""),
        )
        .await?;
        for (holder, parameter) in members {
            security::add_member(&state.db, level.id, holder, parameter.as_deref()).await?;
        }
        if request.is_default {
            scheme.default_level_id = Some(level.id);
            security::update_scheme(&state.db, &scheme).await?;
        }
    }
    Ok(())
}

/// `POST /issuesecurityschemes`: a scheme, optionally with levels. Site administrators only.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::field("name", "The scheme needs a name."))?;
    let taken = security::schemes(&state.db)
        .await?
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(name));
    if taken {
        return Err(ApiError::field(
            "name",
            format!("An issue security scheme named '{name}' already exists."),
        ));
    }
    let scheme = security::create_scheme(
        &state.db,
        name,
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    if let Err(error) = add_levels(&state, &scheme, &request.levels).await {
        security::delete_scheme(&state.db, scheme.id).await?;
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": scheme.id.to_string() })),
    ))
}

/// `PUT /issuesecurityschemes/{id}`: name and description.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<StatusCode> {
    let mut scheme = find_scheme(&state, id).await?;
    if let Some(name) = request.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::field("name", "The scheme needs a name."));
        }
        scheme.name = name.to_string();
    }
    if let Some(description) = request.description {
        scheme.description = description;
    }
    security::update_scheme(&state.db, &scheme).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /issuesecurityschemes/{id}`. A scheme that projects use cannot be deleted.
pub async fn delete(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, id).await?;
    if !security::projects_of(&state.db, scheme.id)
        .await?
        .is_empty()
    {
        return Err(ApiError::bad_request(
            "The issue security scheme is used by projects and cannot be deleted.",
        ));
    }
    security::delete_scheme(&state.db, scheme.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LevelsRequest {
    #[serde(default)]
    levels: Vec<LevelRequest>,
}

/// `PUT /issuesecurityschemes/{id}/level`: adds levels.
pub async fn add_levels_to_scheme(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<LevelsRequest>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, id).await?;
    add_levels(&state, &scheme, &request.levels).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /issuesecurityschemes/{schemeId}/level/{levelId}`: name and description.
pub async fn update_level(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((scheme_id, level_id)): Path<(i64, i64)>,
    Json(request): Json<LevelRequest>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, scheme_id).await?;
    let mut level = find_level(&state, &scheme, level_id).await?;
    if let Some(name) = request.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::bad_request("A security level needs a name."));
        }
        level.name = name.to_string();
    }
    if let Some(description) = request.description {
        level.description = description;
    }
    security::update_level(&state.db, &level).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteLevelQuery {
    replace_with: Option<i64>,
}

/// `DELETE /issuesecurityschemes/{schemeId}/level/{levelId}?replaceWith=`: issues at the level
/// move to `replaceWith`, or lose their level.
pub async fn delete_level(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((scheme_id, level_id)): Path<(i64, i64)>,
    Query(query): Query<DeleteLevelQuery>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, scheme_id).await?;
    let level = find_level(&state, &scheme, level_id).await?;
    if let Some(replacement) = query.replace_with {
        if replacement == level.id {
            return Err(ApiError::bad_request(
                "A level cannot be replaced with itself.",
            ));
        }
        find_level(&state, &scheme, replacement).await?;
    }
    security::delete_level(&state.db, level.id, query.replace_with).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultValue {
    issue_security_scheme_id: String,
    default_level_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsRequest {
    default_values: Vec<DefaultValue>,
}

/// `PUT /issuesecurityschemes/level/default`: the default level of each scheme named.
pub async fn set_defaults(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<DefaultsRequest>,
) -> ApiResult<StatusCode> {
    let parse = |text: &str| {
        text.parse::<i64>()
            .map_err(|_| ApiError::bad_request(format!("'{text}' is not a valid id.")))
    };
    let mut changes = Vec::new();
    for value in &request.default_values {
        let mut scheme = find_scheme(&state, parse(&value.issue_security_scheme_id)?).await?;
        let level = find_level(&state, &scheme, parse(&value.default_level_id)?).await?;
        scheme.default_level_id = Some(level.id);
        changes.push(scheme);
    }
    for scheme in changes {
        security::update_scheme(&state.db, &scheme).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelQuery {
    scheme_id: Option<i64>,
    level_id: Option<i64>,
    #[serde(default)]
    only_default: bool,
}

/// `GET /issuesecurityschemes/level?schemeId=&onlyDefault=`: levels, as a page.
pub async fn levels(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Query(query): Query<LevelQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let schemes = security::schemes(&state.db).await?;
    let mut values = Vec::new();
    for scheme in schemes
        .iter()
        .filter(|s| query.scheme_id.is_none_or(|id| id == s.id))
    {
        for level in security::levels(&state.db, scheme.id).await? {
            let is_default = scheme.default_level_id == Some(level.id);
            if query.only_default && !is_default {
                continue;
            }
            values.push(security_level_json(&ctx, &level, Some(is_default)));
        }
    }
    Ok(Json(paged(
        &ctx,
        "/issuesecurityschemes/level",
        values,
        &page,
    )))
}

fn paged(ctx: &Ctx, path: &str, values: Vec<Value>, page: &Page) -> Value {
    let (start_at, max_results) = page.bounded(100);
    let total = values.len() as i64;
    let values: Vec<Value> = values
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    json!({
        "self": ctx.api_url(path),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })
}

fn member_json(member: &Member, scheme_id: i64) -> Value {
    let mut holder = json!({ "type": member.holder_type, "expand": "group,user,projectRole" });
    if let Some(parameter) = &member.holder_parameter {
        holder["parameter"] = json!(parameter);
        holder["value"] = json!(parameter);
    }
    json!({
        "id": member.id.to_string(),
        "issueSecurityLevelId": member.level_id.to_string(),
        "issueSecuritySchemeId": scheme_id.to_string(),
        "holder": holder,
    })
}

/// `GET /issuesecurityschemes/level/member?schemeId=&levelId=`: level members, as a page.
pub async fn members(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Query(query): Query<LevelQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let mut values = Vec::new();
    for scheme in security::schemes(&state.db).await? {
        if query.scheme_id.is_some_and(|id| id != scheme.id) {
            continue;
        }
        for level in security::levels(&state.db, scheme.id).await? {
            if query.level_id.is_some_and(|id| id != level.id) {
                continue;
            }
            for member in security::members(&state.db, level.id).await? {
                values.push(member_json(&member, scheme.id));
            }
        }
    }
    Ok(Json(paged(
        &ctx,
        "/issuesecurityschemes/level/member",
        values,
        &page,
    )))
}

#[derive(Deserialize)]
pub struct MembersRequest {
    members: Vec<MemberRequest>,
}

/// `PUT /issuesecurityschemes/{schemeId}/level/{levelId}/member`: adds members.
pub async fn add_members(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((scheme_id, level_id)): Path<(i64, i64)>,
    Json(request): Json<MembersRequest>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, scheme_id).await?;
    let level = find_level(&state, &scheme, level_id).await?;
    let mut checked = Vec::new();
    for member in &request.members {
        checked.push(member_holder(&state, member).await?);
    }
    for (holder, parameter) in checked {
        security::add_member(&state.db, level.id, holder, parameter.as_deref()).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /issuesecurityschemes/{schemeId}/level/{levelId}/member/{memberId}`.
pub async fn delete_member(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((scheme_id, level_id, member_id)): Path<(i64, i64, i64)>,
) -> ApiResult<StatusCode> {
    let scheme = find_scheme(&state, scheme_id).await?;
    let level = find_level(&state, &scheme, level_id).await?;
    if !security::delete_member(&state.db, level.id, member_id).await? {
        return Err(ApiError::not_found(format!(
            "The member with id {member_id} does not exist in the level."
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectQuery {
    project_id: Option<i64>,
}

/// `GET /issuesecurityschemes/project?projectId=`: which projects use which scheme.
pub async fn project_associations(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Query(query): Query<ProjectQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let mut values = Vec::new();
    for project in projects::list(&state.db).await? {
        if query.project_id.is_some_and(|id| id != project.id) {
            continue;
        }
        if let Some(scheme) = security::project_scheme(&state.db, project.id).await? {
            values.push(json!({
                "issueSecuritySchemeId": scheme.to_string(),
                "projectId": project.id.to_string(),
            }));
        }
    }
    Ok(Json(paged(
        &ctx,
        "/issuesecurityschemes/project",
        values,
        &page,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelMapping {
    old_level_id: String,
    new_level_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociateRequest {
    /// A scheme id, or `-1` (or null) to remove the project's scheme.
    scheme_id: Option<Value>,
    project_id: Value,
    #[serde(default)]
    old_to_new_security_level_mappings: Vec<LevelMapping>,
}

fn id_of(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

/// `PUT /issuesecurityschemes/project`: gives a project a scheme, moving its issues' levels
/// as the mappings say. Every level issues use needs a mapping, unless the scheme is removed.
pub async fn associate(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(request): Json<AssociateRequest>,
) -> ApiResult<StatusCode> {
    let project_id = id_of(&request.project_id)
        .ok_or_else(|| ApiError::bad_request("projectId is required."))?;
    let project = projects::by_id(&state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::project_not_found(&project_id.to_string()))?;
    let scheme_id = request
        .scheme_id
        .as_ref()
        .and_then(id_of)
        .filter(|id| *id >= 0);
    let scheme = match scheme_id {
        Some(id) => Some(find_scheme(&state, id).await?),
        None => None,
    };
    // `-1` (or nothing) as the new level means "no level".
    let mut mapping = Vec::new();
    for item in &request.old_to_new_security_level_mappings {
        let old = item
            .old_level_id
            .parse::<i64>()
            .map_err(|_| ApiError::bad_request("Level mappings need valid ids."))?;
        let new = match item.new_level_id.trim() {
            "" | "-1" => None,
            text => Some(
                text.parse::<i64>()
                    .map_err(|_| ApiError::bad_request("Level mappings need valid ids."))?,
            ),
        };
        match (&scheme, new) {
            (Some(scheme), Some(new)) => {
                find_level(&state, scheme, new).await?;
            }
            (None, Some(_)) => {
                return Err(ApiError::bad_request(
                    "Without a scheme, issues can only lose their levels.",
                ));
            }
            _ => {}
        }
        mapping.push((old, new));
    }
    // As in Jira, moving to another scheme needs a new level for each level issues use.
    // Removing the scheme clears every level.
    let current = security::project_scheme(&state.db, project.id).await?;
    if let Some(scheme) = &scheme
        && current != Some(scheme.id)
    {
        let unmapped: Vec<i64> = security::levels_in_use(&state.db, project.id)
            .await?
            .into_iter()
            .filter(|level| !mapping.iter().any(|(old, _)| old == level))
            .collect();
        if !unmapped.is_empty() {
            let ids: Vec<String> = unmapped.iter().map(i64::to_string).collect();
            return Err(ApiError::bad_request(format!(
                "Issues in the project use security levels {}. Map each to a level of the new scheme, or to -1 for none.",
                ids.join(", ")
            )));
        }
    }
    security::set_project_scheme(&state.db, project.id, scheme.map(|s| s.id), &mapping).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn browsable_project(
    state: &AppState,
    user: &User,
    key: &str,
) -> ApiResult<(Project, Access)> {
    let project = projects::by_id_or_key(&state.db, key)
        .await?
        .ok_or_else(|| ApiError::project_not_found(key))?;
    let access = require_project(state, user, &project, "BROWSE_PROJECTS").await?;
    Ok((project, access))
}

/// `GET /project/{projectKeyOrId}/issuesecuritylevelscheme`: the project's scheme, with levels.
pub async fn project_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let (project, access) = browsable_project(&state, &user, &key).await?;
    if !access.can("ADMINISTER_PROJECTS") {
        return Err(ApiError::forbidden(
            "You do not have permission to view the issue security scheme of this project.",
        ));
    }
    let scheme_id = security::project_scheme(&state.db, project.id)
        .await?
        .ok_or_else(|| ApiError::not_found("The project has no issue security scheme."))?;
    let scheme = find_scheme(&state, scheme_id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(scheme_json(&ctx, &scheme, true).await?))
}

/// `GET /project/{projectKeyOrId}/securitylevel`: the levels the user may set on the project's
/// issues.
pub async fn project_levels(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let (_, access) = browsable_project(&state, &user, &key).await?;
    let ctx = Ctx::new(&state, version).await?;
    let levels: Vec<Value> = if access.can("SET_ISSUE_SECURITY") {
        access
            .settable_levels(None)
            .iter()
            .map(|l| security_level_json(&ctx, l, None))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Json(json!({ "levels": levels })))
}

/// `GET /securitylevel/{id}`.
pub async fn level(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let level = security::level(&state.db, id).await?.ok_or_else(|| {
        ApiError::not_found(format!("The security level with id {id} does not exist."))
    })?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(security_level_json(&ctx, &level, None)))
}
