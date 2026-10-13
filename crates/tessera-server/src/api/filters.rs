//! Saved filters: `/filter`, `/filter/search`, `/filter/my`, `/filter/favourite`, and sharing.
//!
//! A filter is visible to its owner and, if shared, to the people it is shared with. Today
//! every share type reaches all signed-in users, because Tessera has no groups or project
//! roles yet. Only the owner may edit a filter.

use super::extract::Json;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::render::Ctx;
use super::search::run_jql;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::filters::{self, Filter, Share};
use crate::store::projects;
use crate::store::users::User;

/// The filters a user may see: their own and shared ones.
pub async fn visible(db: &SqlitePool, user: Option<&User>) -> ApiResult<Vec<Filter>> {
    let mut result = Vec::new();
    for filter in filters::all(db).await? {
        let own = user.is_some_and(|u| u.id == filter.owner_id);
        if own || !filters::shares(db, filter.id).await?.is_empty() {
            result.push(filter);
        }
    }
    Ok(result)
}

async fn share_json(ctx: &Ctx, share: &Share, index: usize) -> ApiResult<Value> {
    let mut value = json!({ "id": index + 1, "type": share.share_type });
    if let Some(project_id) = share.project_id
        && let Some(project) = projects::by_id(&ctx.state.db, project_id).await?
    {
        value["project"] = ctx.project_ref(&project);
    }
    Ok(value)
}

pub async fn filter_json(ctx: &mut Ctx, filter: &Filter, viewer: &User) -> ApiResult<Value> {
    let db = ctx.state.db.clone();
    let (favourite, favourite_count) = filters::favourite_state(&db, filter.id, viewer.id).await?;
    let mut shares = Vec::new();
    for (index, share) in filters::shares(&db, filter.id).await?.iter().enumerate() {
        shares.push(share_json(ctx, share, index).await?);
    }
    let encoded_jql: String = form_urlencoded::byte_serialize(filter.jql.as_bytes()).collect();
    Ok(json!({
        "self": ctx.api_url(&format!("/filter/{}", filter.id)),
        "id": filter.id.to_string(),
        "name": filter.name,
        "description": filter.description,
        "owner": ctx.user_ref(Some(filter.owner_id)).await?,
        "jql": filter.jql,
        "viewUrl": format!("{}/issues/?filter={}", ctx.state.base_url, filter.id),
        "searchUrl": ctx.api_url(&format!("/search?jql={encoded_jql}")),
        "favourite": favourite,
        "favouritedCount": favourite_count,
        "sharePermissions": shares,
        "editPermissions": [],
        "isWritable": filter.owner_id == viewer.id,
        "subscriptions": { "size": 0, "items": [], "max-results": 0, "start-index": 0, "end-index": 0 },
    }))
}

async fn find_visible(state: &AppState, user: &User, id: &str) -> ApiResult<Filter> {
    let not_found = || {
        ApiError::bad_request(
            "The selected filter is not available to you, perhaps it has been deleted or had its permissions changed.",
        )
    };
    let id: i64 = id.parse().map_err(|_| not_found())?;
    visible(&state.db, Some(user))
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(not_found)
}

fn check_owner(user: &User, filter: &Filter) -> ApiResult<()> {
    if filter.owner_id == user.id {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "You do not have permission to edit this filter.",
        ))
    }
}

/// Parses `sharePermissions` from a request.
async fn parse_shares(state: &AppState, value: Option<&Value>) -> ApiResult<Vec<Share>> {
    let mut shares = Vec::new();
    for item in value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let share_type = match item.get("type").and_then(Value::as_str) {
            Some("global") => "global",
            Some("loggedin" | "authenticated") => "loggedin",
            Some("project") => "project",
            other => {
                return Err(ApiError::field(
                    "sharePermissions",
                    format!("The share type {other:?} is not supported."),
                ));
            }
        };
        let project_id = if share_type == "project" {
            let reference = item
                .get("project")
                .and_then(|p| p.get("id").or_else(|| p.get("key")))
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .ok_or_else(|| {
                    ApiError::field("sharePermissions", "A project share needs a project.")
                })?;
            let project = projects::by_id_or_key(&state.db, &reference)
                .await?
                .ok_or_else(|| {
                    ApiError::field(
                        "sharePermissions",
                        format!("The project '{reference}' does not exist."),
                    )
                })?;
            Some(project.id)
        } else {
            None
        };
        shares.push(Share {
            share_type: share_type.into(),
            project_id,
        });
    }
    Ok(shares)
}

/// Checks that JQL is valid for this user, reporting problems under `jql` as Jira does.
async fn check_jql(state: &AppState, user: &User, jql: &str) -> ApiResult<()> {
    match run_jql(&state.db, Some(user), jql, 0, 0).await {
        Ok(_) => Ok(()),
        Err(error) if error.status == StatusCode::BAD_REQUEST => {
            Err(ApiError::field("jql", error.error_messages.join(" ")))
        }
        Err(error) => Err(error),
    }
}

async fn check_name(
    state: &AppState,
    user: &User,
    name: &str,
    except: Option<i64>,
) -> ApiResult<()> {
    let taken = filters::all(&state.db).await?.iter().any(|f| {
        f.owner_id == user.id && f.name.eq_ignore_ascii_case(name) && Some(f.id) != except
    });
    if taken {
        return Err(ApiError::field(
            "filterName",
            "Filter with same name already exists.",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterRequest {
    name: Option<String>,
    description: Option<String>,
    jql: Option<String>,
    favourite: Option<bool>,
    share_permissions: Option<Value>,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<FilterRequest>,
) -> ApiResult<Json<Value>> {
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| {
            ApiError::field(
                "filterName",
                "You must specify a name to save this filter as.",
            )
        })?;
    let jql = request.jql.as_deref().unwrap_or("").trim();
    check_name(&state, &user, name, None).await?;
    check_jql(&state, &user, jql).await?;
    let shares = parse_shares(&state, request.share_permissions.as_ref()).await?;
    let filter = filters::create(
        &state.db,
        user.id,
        name,
        request.description.as_deref().unwrap_or(""),
        jql,
    )
    .await?;
    filters::set_shares(&state.db, filter.id, &shares).await?;
    if request.favourite.unwrap_or(false) {
        filters::set_favourite(&state.db, filter.id, user.id, true).await?;
    }
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(filter_json(&mut ctx, &filter, &user).await?))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let filter = find_visible(&state, &user, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(filter_json(&mut ctx, &filter, &user).await?))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<FilterRequest>,
) -> ApiResult<Json<Value>> {
    let mut filter = find_visible(&state, &user, &id).await?;
    check_owner(&user, &filter)?;
    if let Some(name) = request.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::field(
                "filterName",
                "You must specify a name to save this filter as.",
            ));
        }
        check_name(&state, &user, name, Some(filter.id)).await?;
        filter.name = name.to_string();
    }
    if let Some(description) = request.description {
        filter.description = description;
    }
    if let Some(jql) = request.jql {
        check_jql(&state, &user, jql.trim()).await?;
        filter.jql = jql.trim().to_string();
    }
    filters::update(&state.db, &filter).await?;
    if request.share_permissions.is_some() {
        let shares = parse_shares(&state, request.share_permissions.as_ref()).await?;
        filters::set_shares(&state.db, filter.id, &shares).await?;
    }
    if let Some(favourite) = request.favourite {
        filters::set_favourite(&state.db, filter.id, user.id, favourite).await?;
    }
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(filter_json(&mut ctx, &filter, &user).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let filter = find_visible(&state, &user, &id).await?;
    if filter.owner_id != user.id && !user.is_admin {
        return Err(ApiError::bad_request(
            "You do not have permission to delete this filter.",
        ));
    }
    if filters::used_by_board(&state.db, filter.id).await? {
        return Err(ApiError::bad_request(
            "This filter is used by a board and cannot be deleted.",
        ));
    }
    filters::delete(&state.db, filter.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn render_all(ctx: &mut Ctx, list: &[Filter], user: &User) -> ApiResult<Vec<Value>> {
    let mut rendered = Vec::with_capacity(list.len());
    for filter in list {
        rendered.push(filter_json(ctx, filter, user).await?);
    }
    Ok(rendered)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    filter_name: Option<String>,
    account_id: Option<String>,
}

/// Visible filters whose name contains `filterName`, optionally owned by `accountId`.
pub async fn search(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let owner_id = match &query.account_id {
        Some(account_id) => crate::store::users::by_account_id(&state.db, account_id)
            .await?
            .map(|u| u.id)
            .or(Some(-1)),
        None => None,
    };
    let needle = query.filter_name.unwrap_or_default().to_lowercase();
    let matching: Vec<Filter> = visible(&state.db, Some(&user))
        .await?
        .into_iter()
        .filter(|f| f.name.to_lowercase().contains(&needle))
        .filter(|f| owner_id.is_none_or(|owner| f.owner_id == owner))
        .collect();
    let (start_at, max_results) = page.bounded(100);
    let total = matching.len() as i64;
    let page_items: Vec<Filter> = matching
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    let mut ctx = Ctx::new(&state, version).await?;
    let values = render_all(&mut ctx, &page_items, &user).await?;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/filter/search?startAt={start_at}&maxResults={max_results}")),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyQuery {
    #[serde(default)]
    include_favourites: bool,
}

/// The caller's own filters, plus favourites if asked.
pub async fn my(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<MyQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let mut list: Vec<Filter> = filters::all(&state.db)
        .await?
        .into_iter()
        .filter(|f| f.owner_id == user.id)
        .collect();
    if query.include_favourites {
        for favourite in filters::favourites(&state.db, user.id).await? {
            if !list.iter().any(|f| f.id == favourite.id) {
                list.push(favourite);
            }
        }
    }
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(render_all(&mut ctx, &list, &user).await?))
}

pub async fn favourites(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<Json<Vec<Value>>> {
    let visible_ids: Vec<i64> = visible(&state.db, Some(&user))
        .await?
        .iter()
        .map(|f| f.id)
        .collect();
    let list: Vec<Filter> = filters::favourites(&state.db, user.id)
        .await?
        .into_iter()
        .filter(|f| visible_ids.contains(&f.id))
        .collect();
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(render_all(&mut ctx, &list, &user).await?))
}

async fn set_favourite(
    state: AppState,
    version: ApiVersion,
    user: User,
    id: String,
    favourite: bool,
) -> ApiResult<Json<Value>> {
    let filter = find_visible(&state, &user, &id).await?;
    filters::set_favourite(&state.db, filter.id, user.id, favourite).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(filter_json(&mut ctx, &filter, &user).await?))
}

pub async fn add_favourite(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    set_favourite(state, version, user, id, true).await
}

pub async fn remove_favourite(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    set_favourite(state, version, user, id, false).await
}

pub async fn permissions(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let filter = find_visible(&state, &user, &id).await?;
    let ctx = Ctx::new(&state, version).await?;
    let mut shares = Vec::new();
    for (index, share) in filters::shares(&state.db, filter.id)
        .await?
        .iter()
        .enumerate()
    {
        shares.push(share_json(&ctx, share, index).await?);
    }
    Ok(Json(shares))
}
