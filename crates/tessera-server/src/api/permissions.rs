//! Permissions, permission schemes, project roles, and groups, in Jira's REST shapes.

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::access::{
    Access, GLOBAL_PERMISSIONS, HOLDER_TYPES, PROJECT_PERMISSIONS, is_permission, require_project,
};
use super::extract::Json;
use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::store::groups::{self, Group};
use crate::store::issues;
use crate::store::meta::json_id;
use crate::store::permissions::{self, DEFAULT_SCHEME, Grant, Role, Scheme};
use crate::store::projects::{self, Project};
use crate::store::users::{self, User};

fn permission_json(key: &str, name: &str, kind: &str, description: &str) -> Value {
    json!({ "key": key, "name": name, "type": kind, "description": description })
}

/// `GET /permissions`: every permission Tessera knows.
pub async fn all(CurrentUser(_): CurrentUser) -> Json<Value> {
    let mut out = Map::new();
    for (key, name, description) in GLOBAL_PERMISSIONS {
        out.insert(
            (*key).into(),
            permission_json(key, name, "GLOBAL", description),
        );
    }
    for (key, name, description) in PROJECT_PERMISSIONS {
        out.insert(
            (*key).into(),
            permission_json(key, name, "PROJECT", description),
        );
    }
    Json(json!({ "permissions": out }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyPermissionsQuery {
    project_key: Option<String>,
    project_id: Option<String>,
    issue_key: Option<String>,
    issue_id: Option<String>,
    permissions: Option<String>,
}

/// `GET /mypermissions`: which permissions the caller has, globally or in a project or issue.
pub async fn mine(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<MyPermissionsQuery>,
) -> ApiResult<Json<Value>> {
    let asked: Vec<String> = query
        .permissions
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("The permissions parameter is required."))?
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let issue = match query.issue_key.as_ref().or(query.issue_id.as_ref()) {
        Some(reference) => Some(
            issues::by_id_or_key(&state.db, reference)
                .await?
                .ok_or_else(ApiError::issue_not_found)?,
        ),
        None => None,
    };
    let project = match (
        &issue,
        query.project_key.as_ref().or(query.project_id.as_ref()),
    ) {
        (Some(issue), _) => projects::by_id(&state.db, issue.project_id).await?,
        (None, Some(reference)) => Some(
            projects::by_id_or_key(&state.db, reference)
                .await?
                .ok_or_else(|| ApiError::project_not_found(reference))?,
        ),
        (None, None) => None,
    };
    let access = match &project {
        Some(project) => Some(Access::load(&state.db, &user, project).await?),
        None => None,
    };
    let mut out = Map::new();
    for key in &asked {
        let (name, kind, description, have) = if let Some((_, name, description)) =
            GLOBAL_PERMISSIONS.iter().find(|(k, _, _)| k == key)
        {
            let have =
                user.is_admin || matches!(key.as_str(), "USER_PICKER" | "CREATE_SHARED_OBJECTS");
            (*name, "GLOBAL", *description, have)
        } else if let Some((_, name, description)) =
            PROJECT_PERMISSIONS.iter().find(|(k, _, _)| k == key)
        {
            // Without a project, the answer is whether the user has it in any project.
            let have = match &access {
                Some(access) => access.can_on(key, issue.as_ref()),
                None => any_project(&state, &user, key).await?,
            };
            (*name, "PROJECT", *description, have)
        } else {
            return Err(ApiError::bad_request(format!(
                "Unrecognized permission: {key}"
            )));
        };
        let mut value = permission_json(key, name, kind, description);
        value["id"] = json!(key);
        value["havePermission"] = json!(have);
        out.insert(key.clone(), value);
    }
    Ok(Json(json!({ "permissions": out })))
}

async fn any_project(state: &AppState, user: &User, permission: &str) -> ApiResult<bool> {
    for project in projects::list(&state.db).await? {
        if Access::load(&state.db, user, &project)
            .await?
            .can(permission)
        {
            return Ok(true);
        }
    }
    Ok(user.is_admin)
}

// Groups.

fn group_ref(ctx: &Ctx, group: &Group) -> Value {
    json!({
        "name": group.name,
        "groupId": group.group_id,
        "self": ctx.api_url(&format!("/group?groupId={}", group.group_id)),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupQuery {
    groupname: Option<String>,
    group_id: Option<String>,
    account_id: Option<String>,
    query: Option<String>,
}

async fn find_group(state: &AppState, query: &GroupQuery) -> ApiResult<Group> {
    let reference = query
        .group_id
        .as_ref()
        .or(query.groupname.as_ref())
        .ok_or_else(|| ApiError::bad_request("A groupname or groupId is required."))?;
    groups::find(&state.db, reference)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Specified group does not exist: {reference}")))
}

async fn group_json(ctx: &mut Ctx, state: &AppState, group: &Group) -> ApiResult<Value> {
    let mut value = group_ref(ctx, group);
    let members = groups::members(&state.db, group.id).await?;
    let mut items = Vec::new();
    for id in &members {
        if let Some(user) = users::by_id(&state.db, *id).await? {
            items.push(ctx.user(&user));
        }
    }
    value["users"] = json!({ "size": items.len(), "items": items, "max-results": 50, "start-index": 0, "end-index": 0 });
    value["expand"] = json!("users");
    Ok(value)
}

#[derive(Deserialize)]
pub struct NewGroup {
    name: String,
}

pub async fn create_group(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<NewGroup>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("You must specify a group name."));
    }
    if groups::find(&state.db, name).await?.is_some() {
        return Err(ApiError::bad_request(
            "A group with that name already exists.",
        ));
    }
    let group = groups::create(&state.db, name).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(group_json(&mut ctx, &state, &group).await?),
    ))
}

pub async fn delete_group(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(query): Query<GroupQuery>,
) -> ApiResult<StatusCode> {
    let group = find_group(&state, &query).await?;
    groups::delete(&state.db, group.id).await?;
    Ok(StatusCode::OK)
}

pub async fn get_group(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<GroupQuery>,
) -> ApiResult<Json<Value>> {
    let group = find_group(&state, &query).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(group_json(&mut ctx, &state, &group).await?))
}

/// `GET /group/member`: one page of a group's members.
pub async fn members(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<GroupQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let group = find_group(&state, &query).await?;
    let ctx = Ctx::new(&state, version).await?;
    let ids = groups::members(&state.db, group.id).await?;
    let (start_at, max_results) = page.bounded(50);
    let mut values = Vec::new();
    for id in ids
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
    {
        if let Some(user) = users::by_id(&state.db, *id).await? {
            values.push(ctx.user(&user));
        }
    }
    let total = ids.len() as i64;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/group/member?groupId={}", group.group_id)),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRequest {
    account_id: Option<String>,
    /// Jira Server names users by username; Tessera matches it to an email or account id.
    name: Option<String>,
}

pub async fn add_member(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Query(query): Query<GroupQuery>,
    Json(request): Json<MemberRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let group = find_group(&state, &query).await?;
    let user = match (&request.account_id, &request.name) {
        (Some(account_id), _) => find_user(&state, account_id).await?,
        (None, Some(name)) => match users::by_email(&state.db, name).await? {
            Some(user) => user,
            None => find_user(&state, name).await?,
        },
        (None, None) => return Err(ApiError::bad_request("The accountId is required.")),
    };
    groups::add_member(&state.db, group.id, user.id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(group_json(&mut ctx, &state, &group).await?),
    ))
}

pub async fn remove_member(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(query): Query<GroupQuery>,
) -> ApiResult<StatusCode> {
    let group = find_group(&state, &query).await?;
    let account_id = query
        .account_id
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("The accountId parameter is required."))?;
    let user = find_user(&state, account_id).await?;
    groups::remove_member(&state.db, group.id, user.id).await?;
    Ok(StatusCode::OK)
}

/// `GET /groups/picker`: groups whose name contains `query`.
pub async fn picker(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<GroupQuery>,
) -> ApiResult<Json<Value>> {
    let needle = query.query.unwrap_or_default().to_lowercase();
    let found: Vec<Value> = groups::all(&state.db)
        .await?
        .into_iter()
        .filter(|g| g.name.to_lowercase().contains(&needle))
        .map(|g| json!({ "name": g.name, "html": g.name, "groupId": g.group_id, "labels": [] }))
        .collect();
    Ok(Json(json!({
        "header": format!("Showing {} of {} matching groups", found.len(), found.len()),
        "total": found.len(),
        "groups": found,
    })))
}

/// `GET /group/bulk`: every group, a page at a time.
pub async fn bulk(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = groups::all(&state.db).await?;
    let (start_at, max_results) = page.bounded(50);
    let values: Vec<Value> = all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .map(|g| json!({ "name": g.name, "groupId": g.group_id }))
        .collect();
    let total = all.len() as i64;
    Ok(Json(json!({
        "self": ctx.api_url("/group/bulk"),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

/// `GET /user/groups`: the groups of one user.
pub async fn user_groups(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<GroupQuery>,
) -> ApiResult<Json<Value>> {
    let account_id = query
        .account_id
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("The accountId parameter is required."))?;
    let user = find_user(&state, account_id).await?;
    let ctx = Ctx::new(&state, version).await?;
    let list = groups::of_user(&state.db, user.id).await?;
    Ok(Json(json!(
        list.iter().map(|g| group_ref(&ctx, g)).collect::<Vec<_>>()
    )))
}

async fn find_user(state: &AppState, account_id: &str) -> ApiResult<User> {
    users::by_account_id(&state.db, account_id)
        .await?
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "The user with account id {account_id} does not exist."
            ))
        })
}

// Project roles.

fn role_ref(ctx: &Ctx, role: &Role, project: Option<&Project>) -> Value {
    let path = match project {
        Some(p) => format!("/project/{}/role/{}", p.key, role.id),
        None => format!("/role/{}", role.id),
    };
    json!({
        "self": ctx.api_url(&path),
        "name": role.name,
        "id": role.id,
        "description": role.description,
    })
}

/// A role with its actors in a project.
async fn role_json(
    ctx: &mut Ctx,
    state: &AppState,
    role: &Role,
    project: &Project,
) -> ApiResult<Value> {
    let mut value = role_ref(ctx, role, Some(project));
    let mut actors = Vec::new();
    for id in permissions::role_users(&state.db, project.id, role.id).await? {
        if let Some(user) = users::by_id(&state.db, id).await? {
            actors.push(json!({
                "id": user.id,
                "displayName": user.display_name,
                "type": "atlassian-user-role-actor",
                "actorUser": { "accountId": user.account_id },
            }));
        }
    }
    for id in permissions::role_groups(&state.db, project.id, role.id).await? {
        if let Some(group) = groups::by_id(&state.db, id).await? {
            actors.push(json!({
                "id": group.id,
                "displayName": group.name,
                "type": "atlassian-group-role-actor",
                "name": group.name,
                "actorGroup": { "name": group.name, "displayName": group.name, "groupId": group.group_id },
            }));
        }
    }
    value["actors"] = json!(actors);
    value["scope"] = json!({ "type": "PROJECT", "project": { "id": project.id.to_string() } });
    Ok(value)
}

async fn find_role(state: &AppState, id: i64) -> ApiResult<Role> {
    permissions::role(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Project role with id {id} does not exist.")))
}

async fn find_project(
    state: &AppState,
    user: &User,
    key: &str,
    permission: &str,
) -> ApiResult<Project> {
    let project = projects::by_id_or_key(&state.db, key)
        .await?
        .ok_or_else(|| ApiError::project_not_found(key))?;
    require_project(state, user, &project, permission).await?;
    Ok(project)
}

/// `GET /role`: every project role.
pub async fn roles(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, version).await?;
    let all = permissions::roles(&state.db).await?;
    Ok(Json(json!(
        all.iter()
            .map(|r| role_ref(&ctx, r, None))
            .collect::<Vec<_>>()
    )))
}

#[derive(Deserialize)]
pub struct RoleRequest {
    name: Option<String>,
    description: Option<String>,
}

pub async fn create_role(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<RoleRequest>,
) -> ApiResult<Json<Value>> {
    let name = request.name.unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::field("name", "A project role name is required."));
    }
    if permissions::roles(&state.db)
        .await?
        .iter()
        .any(|r| r.name.eq_ignore_ascii_case(name))
    {
        return Err(ApiError::field(
            "name",
            "A project role with this name already exists.",
        ));
    }
    let role = permissions::create_role(
        &state.db,
        name,
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_ref(&ctx, &role, None)))
}

pub async fn get_role(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let role = find_role(&state, id).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_ref(&ctx, &role, None)))
}

pub async fn update_role(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<RoleRequest>,
) -> ApiResult<Json<Value>> {
    let role = find_role(&state, id).await?;
    let name = request.name.unwrap_or(role.name);
    let description = request.description.unwrap_or(role.description);
    permissions::update_role(&state.db, id, name.trim(), &description).await?;
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_ref(&ctx, &find_role(&state, id).await?, None)))
}

pub async fn delete_role(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_role(&state, id).await?;
    if id == permissions::ADMINISTRATORS {
        return Err(ApiError::bad_request(
            "The Administrators role cannot be deleted.",
        ));
    }
    permissions::delete_role(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /project/{key}/role`: role names with their URLs.
pub async fn project_roles(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    let mut out = Map::new();
    for role in permissions::roles(&state.db).await? {
        out.insert(
            role.name.clone(),
            json!(ctx.api_url(&format!("/project/{}/role/{}", project.key, role.id))),
        );
    }
    Ok(Json(Value::Object(out)))
}

/// `GET /project/{key}/roledetails`.
pub async fn project_role_details(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    let all = permissions::roles(&state.db).await?;
    Ok(Json(json!(
        all.iter()
            .map(|r| role_ref(&ctx, r, Some(&project)))
            .collect::<Vec<_>>()
    )))
}

pub async fn project_role(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((key, id)): Path<(String, i64)>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "BROWSE_PROJECTS").await?;
    let role = find_role(&state, id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_json(&mut ctx, &state, &role, &project).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorsRequest {
    #[serde(default)]
    user: Vec<String>,
    #[serde(default)]
    group: Vec<String>,
    #[serde(default)]
    group_id: Vec<String>,
    /// `PUT` sends the full list this way.
    categorised_actors: Option<Map<String, Value>>,
}

/// Adds the request's users and groups to a role in a project.
async fn add_actors(
    state: &AppState,
    project: &Project,
    role: &Role,
    request: &ActorsRequest,
) -> ApiResult<()> {
    let mut users_list = request.user.clone();
    let mut groups_list: Vec<String> = request
        .group
        .iter()
        .chain(&request.group_id)
        .cloned()
        .collect();
    if let Some(categorised) = &request.categorised_actors {
        let list = |key: &str| -> Vec<String> {
            categorised
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        users_list.extend(list("atlassian-user-role-actor"));
        groups_list.extend(list("atlassian-group-role-actor"));
        groups_list.extend(list("atlassian-group-role-actor-id"));
    }
    for account_id in &users_list {
        let user = find_user(state, account_id).await?;
        permissions::add_role_user(&state.db, project.id, role.id, user.id).await?;
    }
    for reference in &groups_list {
        let group = groups::find(&state.db, reference).await?.ok_or_else(|| {
            ApiError::bad_request(format!("Specified group does not exist: {reference}"))
        })?;
        permissions::add_role_group(&state.db, project.id, role.id, group.id).await?;
    }
    Ok(())
}

/// `POST /project/{key}/role/{id}`: adds actors to a role. Project administrators only.
pub async fn add_role_actors(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((key, id)): Path<(String, i64)>,
    Json(request): Json<ActorsRequest>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "ADMINISTER_PROJECTS").await?;
    let role = find_role(&state, id).await?;
    add_actors(&state, &project, &role, &request).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_json(&mut ctx, &state, &role, &project).await?))
}

/// `PUT /project/{key}/role/{id}`: replaces a role's actors.
pub async fn set_role_actors(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((key, id)): Path<(String, i64)>,
    Json(request): Json<ActorsRequest>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "ADMINISTER_PROJECTS").await?;
    let role = find_role(&state, id).await?;
    for user_id in permissions::role_users(&state.db, project.id, role.id).await? {
        permissions::remove_role_user(&state.db, project.id, role.id, user_id).await?;
    }
    for group_id in permissions::role_groups(&state.db, project.id, role.id).await? {
        permissions::remove_role_group(&state.db, project.id, role.id, group_id).await?;
    }
    add_actors(&state, &project, &role, &request).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(role_json(&mut ctx, &state, &role, &project).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveActorQuery {
    user: Option<String>,
    group: Option<String>,
    group_id: Option<String>,
}

/// `DELETE /project/{key}/role/{id}?user=...` (or `group`, `groupId`).
pub async fn remove_role_actor(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((key, id)): Path<(String, i64)>,
    Query(query): Query<RemoveActorQuery>,
) -> ApiResult<StatusCode> {
    let project = find_project(&state, &user, &key, "ADMINISTER_PROJECTS").await?;
    let role = find_role(&state, id).await?;
    let removed = if let Some(account_id) = &query.user {
        let target = find_user(&state, account_id).await?;
        permissions::remove_role_user(&state.db, project.id, role.id, target.id).await?
    } else if let Some(reference) = query.group_id.as_ref().or(query.group.as_ref()) {
        match groups::find(&state.db, reference).await? {
            Some(group) => {
                permissions::remove_role_group(&state.db, project.id, role.id, group.id).await?
            }
            None => false,
        }
    } else {
        return Err(ApiError::bad_request(
            "Specify a user or a group to remove.",
        ));
    };
    if !removed {
        return Err(ApiError::not_found("The actor is not in this role."));
    }
    Ok(StatusCode::NO_CONTENT)
}

// Permission schemes.

async fn grant_json(ctx: &mut Ctx, state: &AppState, grant: &Grant) -> ApiResult<Value> {
    let param = grant.holder_parameter.clone();
    let mut holder =
        json!({ "type": grant.holder_type, "parameter": param, "value": param, "expand": "" });
    match grant.holder_type.as_str() {
        "projectRole" => {
            if let Some(role) = match param.as_deref().and_then(|p| p.parse().ok()) {
                Some(id) => permissions::role(&state.db, id).await?,
                None => None,
            } {
                holder["projectRole"] = role_ref(ctx, &role, None);
                holder["expand"] = json!("projectRole");
            }
        }
        "user" => {
            if let Some(user) = match param.as_deref() {
                Some(id) => users::by_account_id(&state.db, id).await?,
                None => None,
            } {
                holder["user"] = ctx.user(&user);
                holder["expand"] = json!("user");
            }
        }
        "group" => {
            if let Some(group) = match param.as_deref() {
                Some(name) => groups::find(&state.db, name).await?,
                None => None,
            } {
                holder["group"] = group_ref(ctx, &group);
                holder["value"] = json!(group.group_id);
                holder["expand"] = json!("group");
            }
        }
        _ => {}
    }
    Ok(json!({
        "id": grant.id,
        "self": ctx.api_url(&format!("/permissionscheme/{}/permission/{}", grant.scheme_id, grant.id)),
        "holder": holder,
        "permission": grant.permission,
    }))
}

async fn scheme_json(
    ctx: &mut Ctx,
    state: &AppState,
    scheme: &Scheme,
    with_grants: bool,
) -> ApiResult<Value> {
    let mut value = json!({
        "expand": "permissions,user,group,projectRole,field,all",
        "id": scheme.id,
        "self": ctx.api_url(&format!("/permissionscheme/{}", scheme.id)),
        "name": scheme.name,
        "description": scheme.description,
    });
    if with_grants {
        let mut list = Vec::new();
        for grant in permissions::grants(&state.db, scheme.id).await? {
            list.push(grant_json(ctx, state, &grant).await?);
        }
        value["permissions"] = json!(list);
    }
    Ok(value)
}

fn wants_grants(expand: &Option<String>) -> bool {
    expand.as_deref().is_some_and(|e| {
        e.split(',')
            .any(|part| matches!(part.trim(), "permissions" | "all"))
    })
}

#[derive(Deserialize)]
pub struct ExpandQuery {
    expand: Option<String>,
}

async fn find_scheme(state: &AppState, id: i64) -> ApiResult<Scheme> {
    permissions::scheme(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Permission scheme {id} does not exist.")))
}

pub async fn schemes(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<ExpandQuery>,
) -> ApiResult<Json<Value>> {
    let mut ctx = Ctx::new(&state, version).await?;
    let with = wants_grants(&query.expand);
    let mut list = Vec::new();
    for scheme in permissions::schemes(&state.db).await? {
        list.push(scheme_json(&mut ctx, &state, &scheme, with).await?);
    }
    Ok(Json(json!({ "permissionSchemes": list })))
}

pub async fn scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
    Query(query): Query<ExpandQuery>,
) -> ApiResult<Json<Value>> {
    let scheme = find_scheme(&state, id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(
        scheme_json(&mut ctx, &state, &scheme, wants_grants(&query.expand)).await?,
    ))
}

#[derive(Deserialize)]
pub struct SchemeRequest {
    name: Option<String>,
    description: Option<String>,
    permissions: Option<Vec<GrantRequest>>,
}

#[derive(Deserialize)]
pub struct GrantRequest {
    holder: Value,
    permission: String,
}

/// Checks a grant and returns its holder type and parameter.
async fn holder_of(
    state: &AppState,
    request: &GrantRequest,
) -> ApiResult<(String, Option<String>)> {
    if !is_permission(&request.permission) {
        return Err(ApiError::bad_request(format!(
            "Unknown permission: {}",
            request.permission
        )));
    }
    let kind = request.holder["type"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if !HOLDER_TYPES.contains(&kind.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Unsupported holder type: {kind}"
        )));
    }
    let param = request.holder["parameter"]
        .as_str()
        .or_else(|| request.holder["value"].as_str())
        .map(str::to_string)
        .or_else(|| json_id(&request.holder["parameter"]).map(|id| id.to_string()));
    match kind.as_str() {
        "projectRole" => {
            let id: i64 = param
                .as_deref()
                .and_then(|p| p.parse().ok())
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "A project role holder needs the role id as its parameter.",
                    )
                })?;
            find_role(state, id).await?;
        }
        "user" => {
            find_user(state, param.as_deref().unwrap_or_default()).await?;
        }
        "group" => {
            let reference = param.as_deref().unwrap_or_default();
            let group = groups::find(&state.db, reference).await?.ok_or_else(|| {
                ApiError::bad_request(format!("Specified group does not exist: {reference}"))
            })?;
            // Grants name groups by name, as Jira's do.
            return Ok((kind, Some(group.name)));
        }
        _ => {}
    }
    Ok((kind, param))
}

pub async fn create_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request.name.unwrap_or_default();
    if name.trim().is_empty() {
        return Err(ApiError::field(
            "name",
            "The permission scheme name is required.",
        ));
    }
    if permissions::schemes(&state.db)
        .await?
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(name.trim()))
    {
        return Err(ApiError::field(
            "name",
            "A permission scheme with this name already exists.",
        ));
    }
    let mut holders = Vec::new();
    for grant in request.permissions.as_deref().unwrap_or_default() {
        holders.push((grant.permission.clone(), holder_of(&state, grant).await?));
    }
    let scheme = permissions::create_scheme(
        &state.db,
        name.trim(),
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    for (permission, (kind, param)) in &holders {
        permissions::add_grant(&state.db, scheme.id, permission, kind, param.as_deref()).await?;
    }
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(scheme_json(&mut ctx, &state, &scheme, true).await?),
    ))
}

/// Updates a scheme's name and description. Given `permissions`, they replace the grants.
pub async fn update_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<SchemeRequest>,
) -> ApiResult<Json<Value>> {
    let scheme = find_scheme(&state, id).await?;
    let name = request.name.unwrap_or(scheme.name);
    let description = request.description.unwrap_or(scheme.description);
    if let Some(grants) = &request.permissions {
        let mut holders = Vec::new();
        for grant in grants {
            holders.push((grant.permission.clone(), holder_of(&state, grant).await?));
        }
        for existing in permissions::grants(&state.db, id).await? {
            permissions::delete_grant(&state.db, id, existing.id).await?;
        }
        for (permission, (kind, param)) in &holders {
            permissions::add_grant(&state.db, id, permission, kind, param.as_deref()).await?;
        }
    }
    permissions::update_scheme(&state.db, id, name.trim(), &description).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(
        scheme_json(&mut ctx, &state, &find_scheme(&state, id).await?, true).await?,
    ))
}

pub async fn delete_scheme(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_scheme(&state, id).await?;
    if id == DEFAULT_SCHEME {
        return Err(ApiError::bad_request(
            "The default permission scheme cannot be deleted.",
        ));
    }
    permissions::delete_scheme(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn grants(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    find_scheme(&state, id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let mut list = Vec::new();
    for grant in permissions::grants(&state.db, id).await? {
        list.push(grant_json(&mut ctx, &state, &grant).await?);
    }
    Ok(Json(json!({ "permissions": list })))
}

pub async fn create_grant(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(_): AdminUser,
    Path(id): Path<i64>,
    Json(request): Json<GrantRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    find_scheme(&state, id).await?;
    let (kind, param) = holder_of(&state, &request).await?;
    let grant =
        permissions::add_grant(&state.db, id, &request.permission, &kind, param.as_deref()).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok((
        StatusCode::CREATED,
        Json(grant_json(&mut ctx, &state, &grant).await?),
    ))
}

pub async fn get_grant(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path((id, grant_id)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    find_scheme(&state, id).await?;
    let grant = permissions::grants(&state.db, id)
        .await?
        .into_iter()
        .find(|g| g.id == grant_id)
        .ok_or_else(|| {
            ApiError::not_found(format!("Permission grant {grant_id} does not exist."))
        })?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(grant_json(&mut ctx, &state, &grant).await?))
}

pub async fn delete_grant(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((id, grant_id)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    if !permissions::delete_grant(&state.db, id, grant_id).await? {
        return Err(ApiError::not_found(format!(
            "Permission grant {grant_id} does not exist."
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /project/{key}/permissionscheme`.
pub async fn project_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Query(query): Query<ExpandQuery>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &user, &key, "ADMINISTER_PROJECTS").await?;
    let id = permissions::project_scheme(&state.db, project.id).await?;
    let scheme = find_scheme(&state, id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(
        scheme_json(&mut ctx, &state, &scheme, wants_grants(&query.expand)).await?,
    ))
}

#[derive(Deserialize)]
pub struct AssignSchemeRequest {
    id: Value,
}

/// `PUT /project/{key}/permissionscheme`: uses another scheme. Site administrators only.
pub async fn assign_project_scheme(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    AdminUser(admin): AdminUser,
    Path(key): Path<String>,
    Json(request): Json<AssignSchemeRequest>,
) -> ApiResult<Json<Value>> {
    let project = find_project(&state, &admin, &key, "BROWSE_PROJECTS").await?;
    let id =
        json_id(&request.id).ok_or_else(|| ApiError::field("id", "The scheme id is required."))?;
    let scheme = find_scheme(&state, id).await?;
    permissions::set_project_scheme(&state.db, project.id, scheme.id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(scheme_json(&mut ctx, &state, &scheme, false).await?))
}
