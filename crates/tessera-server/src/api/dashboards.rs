//! Dashboards: `/dashboard`, `/dashboard/search`, `/dashboard/{id}`, copies, gadgets, and gadget
//! item properties.
//!
//! A dashboard is visible to its owner and to the people it is shared with; the system
//! dashboard is visible to everyone. Only the owner may change a dashboard, and only site
//! administrators may change the system dashboard. Gadgets keep their settings in item
//! properties, as Jira's gadgets do.

use axum::Extension;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::extract::Json;
use super::render::Ctx;
use super::sharing;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::dashboards::{self, Dashboard, Gadget, NewGadget};
use crate::store::users::{self, User};

/// The gadgets Tessera draws, by Jira module key, with their default titles.
pub const GADGETS: &[(&str, &str)] = &[
    (
        "com.atlassian.jira.gadgets:introduction-dashboard-item",
        "Introduction",
    ),
    (
        "com.atlassian.jira.gadgets:assigned-to-me-dashboard-item",
        "Assigned to me",
    ),
    (
        "com.atlassian.jira.gadgets:filter-results-dashboard-item",
        "Filter results",
    ),
    (
        "com.atlassian.jira.gadgets:pie-chart-dashboard-item",
        "Pie chart",
    ),
    (
        "com.atlassian.jira.gadgets:created-vs-resolved-issues-chart-dashboard-item",
        "Created vs. resolved chart",
    ),
    (
        "com.atlassian.jira.gadgets:watched-issues-dashboard-item",
        "Watched issues",
    ),
];

const COLORS: &[&str] = &[
    "blue", "red", "yellow", "green", "cyan", "purple", "gray", "white",
];

const LAYOUTS: &[&str] = &["A", "AA", "AB", "BA", "AAA"];

fn not_found(id: &str) -> ApiError {
    ApiError::not_found(format!("The dashboard with id '{id}' does not exist."))
}

async fn can_view(state: &AppState, user: &User, dashboard: &Dashboard) -> ApiResult<bool> {
    if dashboard.is_system() || dashboard.owner_id == Some(user.id) || user.is_admin {
        return Ok(true);
    }
    let shares = dashboards::shares(&state.db, dashboard.id).await?;
    sharing::reaches(&state.db, Some(user), &shares).await
}

fn can_edit(user: &User, dashboard: &Dashboard) -> bool {
    match dashboard.owner_id {
        Some(owner) => owner == user.id || user.is_admin,
        None => user.is_admin,
    }
}

/// A dashboard the user may see.
async fn find_visible(state: &AppState, user: &User, id: &str) -> ApiResult<Dashboard> {
    let number: i64 = id.parse().map_err(|_| not_found(id))?;
    let dashboard = dashboards::by_id(&state.db, number)
        .await?
        .ok_or_else(|| not_found(id))?;
    if !can_view(state, user, &dashboard).await? {
        return Err(not_found(id));
    }
    Ok(dashboard)
}

/// A dashboard the user may change.
async fn find_editable(state: &AppState, user: &User, id: &str) -> ApiResult<Dashboard> {
    let dashboard = find_visible(state, user, id).await?;
    if !can_edit(user, &dashboard) {
        return Err(ApiError::forbidden(
            "You do not have permission to edit this dashboard.",
        ));
    }
    Ok(dashboard)
}

/// The dashboards a user may see.
pub async fn visible(state: &AppState, user: &User) -> ApiResult<Vec<Dashboard>> {
    let mut out = Vec::new();
    for dashboard in dashboards::all(&state.db).await? {
        if can_view(state, user, &dashboard).await? {
            out.push(dashboard);
        }
    }
    Ok(out)
}

pub async fn dashboard_json(
    ctx: &mut Ctx,
    dashboard: &Dashboard,
    viewer: &User,
) -> ApiResult<Value> {
    let db = ctx.state.db.clone();
    let (favourite, popularity) = dashboards::favourite_state(&db, dashboard.id, viewer.id).await?;
    let shares = sharing::shares_json(ctx, &dashboards::shares(&db, dashboard.id).await?).await?;
    let mut out = json!({
        "id": dashboard.id.to_string(),
        "name": dashboard.name,
        "description": dashboard.description,
        "isFavourite": favourite,
        "popularity": popularity,
        "self": ctx.api_url(&format!("/dashboard/{}", dashboard.id)),
        "view": format!("{}/jira/dashboards/{}", ctx.state.base_url, dashboard.id),
        "sharePermissions": shares,
        "editPermissions": [],
        "isWritable": can_edit(viewer, dashboard),
        "systemDashboard": dashboard.is_system(),
        "automaticRefreshMs": dashboard.refresh_ms.unwrap_or(0),
        // Tessera's own field: the column layout, as Jira names layouts.
        "layout": dashboard.layout,
    });
    if let Some(owner) = dashboard.owner_id {
        out["owner"] = ctx.user_ref(Some(owner)).await?;
    }
    Ok(out)
}

async fn render_all(ctx: &mut Ctx, list: &[Dashboard], user: &User) -> ApiResult<Vec<Value>> {
    let mut out = Vec::with_capacity(list.len());
    for dashboard in list {
        out.push(dashboard_json(ctx, dashboard, user).await?);
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct ListQuery {
    filter: Option<String>,
}

/// `GET /dashboard?filter=my|favourite`.
pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ListQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let mut all = visible(&state, &user).await?;
    match query.filter.as_deref() {
        Some("my") => all.retain(|d| d.owner_id == Some(user.id)),
        Some("favourite") => {
            let mut kept = Vec::new();
            for dashboard in all {
                if dashboards::favourite_state(&state.db, dashboard.id, user.id)
                    .await?
                    .0
                {
                    kept.push(dashboard);
                }
            }
            all = kept;
        }
        Some(other) if !other.is_empty() => {
            return Err(ApiError::bad_request(format!(
                "The filter '{other}' is not valid. Use 'my' or 'favourite'."
            )));
        }
        _ => {}
    }
    let (start_at, max_results) = page.bounded(1000);
    let total = all.len() as i64;
    let items: Vec<Dashboard> = all
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    let mut ctx = Ctx::new(&state, version).await?;
    let mut out = json!({
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "dashboards": render_all(&mut ctx, &items, &user).await?,
    });
    let filter = query
        .filter
        .map(|f| format!("&filter={f}"))
        .unwrap_or_default();
    if start_at > 0 {
        let previous = (start_at - max_results).max(0);
        out["prev"] = json!(ctx.api_url(&format!(
            "/dashboard?startAt={previous}&maxResults={max_results}{filter}"
        )));
    }
    if start_at + max_results < total {
        out["next"] = json!(ctx.api_url(&format!(
            "/dashboard?startAt={}&maxResults={max_results}{filter}",
            start_at + max_results
        )));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    dashboard_name: Option<String>,
    account_id: Option<String>,
    order_by: Option<String>,
}

/// `GET /dashboard/search`: visible dashboards by name and owner.
pub async fn search(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let owner = match &query.account_id {
        Some(account) => Some(
            users::by_account_id(&state.db, account)
                .await?
                .map_or(-1, |u| u.id),
        ),
        None => None,
    };
    let needle = query.dashboard_name.unwrap_or_default().to_lowercase();
    let mut found: Vec<Dashboard> = visible(&state, &user)
        .await?
        .into_iter()
        .filter(|d| d.name.to_lowercase().contains(&needle))
        .filter(|d| owner.is_none_or(|o| d.owner_id == Some(o)))
        .collect();
    let order = query.order_by.unwrap_or_else(|| "name".into());
    let (descending, field) = match order.strip_prefix('-') {
        Some(field) => (true, field),
        None => (false, order.strip_prefix('+').unwrap_or(&order)),
    };
    match field {
        "name" => found.sort_by_key(|d| d.name.to_lowercase()),
        "id" => found.sort_by_key(|d| d.id),
        "owner" => found.sort_by_key(|d| d.owner_id),
        other => {
            return Err(ApiError::bad_request(format!(
                "The orderBy value '{other}' is not supported."
            )));
        }
    }
    if descending {
        found.reverse();
    }
    let (start_at, max_results) = page.bounded(100);
    let total = found.len() as i64;
    let items: Vec<Dashboard> = found
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/dashboard/search?startAt={start_at}&maxResults={max_results}")),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": render_all(&mut ctx, &items, &user).await?,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRequest {
    name: Option<String>,
    description: Option<String>,
    share_permissions: Option<Value>,
}

fn required_name(request: &DashboardRequest) -> ApiResult<String> {
    request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::field("name", "The name of the dashboard must be specified."))
}

/// `POST /dashboard`.
pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<DashboardRequest>,
) -> ApiResult<Json<Value>> {
    let name = required_name(&request)?;
    let shares = sharing::parse(&state, request.share_permissions.as_ref()).await?;
    let dashboard = dashboards::create(
        &state.db,
        user.id,
        &name,
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    dashboards::set_shares(&state.db, dashboard.id, &shares).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(dashboard_json(&mut ctx, &dashboard, &user).await?))
}

/// `GET /dashboard/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let dashboard = find_visible(&state, &user, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(dashboard_json(&mut ctx, &dashboard, &user).await?))
}

/// `PUT /dashboard/{id}`: name, description, and sharing.
pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<DashboardRequest>,
) -> ApiResult<Json<Value>> {
    let mut dashboard = find_editable(&state, &user, &id).await?;
    dashboard.name = required_name(&request)?;
    if let Some(description) = request.description {
        dashboard.description = description;
    }
    dashboards::update(&state.db, &dashboard).await?;
    if request.share_permissions.is_some() && !dashboard.is_system() {
        let shares = sharing::parse(&state, request.share_permissions.as_ref()).await?;
        dashboards::set_shares(&state.db, dashboard.id, &shares).await?;
    }
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(dashboard_json(&mut ctx, &dashboard, &user).await?))
}

/// `DELETE /dashboard/{id}`. The system dashboard stays.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let dashboard = find_editable(&state, &user, &id).await?;
    if dashboard.is_system() {
        return Err(ApiError::bad_request(
            "The system dashboard cannot be deleted.",
        ));
    }
    dashboards::delete(&state.db, dashboard.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /dashboard/{id}/copy`: a new dashboard of the caller's, with the same gadgets.
pub async fn copy(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<DashboardRequest>,
) -> ApiResult<Json<Value>> {
    let original = find_visible(&state, &user, &id).await?;
    let name = required_name(&request)?;
    let shares = sharing::parse(&state, request.share_permissions.as_ref()).await?;
    let description = request
        .description
        .unwrap_or_else(|| original.description.clone());
    let created = dashboards::create(&state.db, user.id, &name, &description).await?;
    dashboards::set_shares(&state.db, created.id, &shares).await?;
    dashboards::copy_contents(&state.db, &original, created.id).await?;
    let copied = dashboards::by_id(&state.db, created.id)
        .await?
        .unwrap_or(created);
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(dashboard_json(&mut ctx, &copied, &user).await?))
}

fn gadget_json(gadget: &Gadget) -> Value {
    let mut out = json!({
        "id": gadget.id,
        "color": gadget.color,
        "position": { "row": gadget.row, "column": gadget.col },
        "title": gadget.title,
    });
    if let Some(key) = &gadget.module_key {
        out["moduleKey"] = json!(key);
    }
    if let Some(uri) = &gadget.uri {
        out["uri"] = json!(uri);
    }
    out
}

/// `GET /dashboard/gadgets`: the gadgets that can be added.
pub async fn available(CurrentUser(_): CurrentUser) -> Json<Value> {
    let gadgets: Vec<Value> = GADGETS
        .iter()
        .map(|(key, title)| json!({ "moduleKey": key, "title": title }))
        .collect();
    Json(json!({ "gadgets": gadgets }))
}

/// `GET /dashboard/{id}/gadget`, optionally narrowed by `moduleKey`, `uri`, or `gadgetId`,
/// each of which may repeat.
pub async fn gadgets(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let pairs: Vec<(String, String)> = form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .into_owned()
        .collect();
    let all = |name: &str| -> Vec<String> {
        pairs
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    };
    let (keys, uris, ids) = (all("moduleKey"), all("uri"), all("gadgetId"));
    let dashboard = find_visible(&state, &user, &id).await?;
    let list: Vec<Value> = dashboards::gadgets(&state.db, dashboard.id)
        .await?
        .iter()
        .filter(|g| keys.is_empty() || g.module_key.as_ref().is_some_and(|k| keys.contains(k)))
        .filter(|g| uris.is_empty() || g.uri.as_ref().is_some_and(|u| uris.contains(u)))
        .filter(|g| ids.is_empty() || ids.contains(&g.id.to_string()))
        .map(gadget_json)
        .collect();
    Ok(Json(json!({ "gadgets": list })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GadgetRequest {
    // The Python client sends `module_key`; Jira's documented name is `moduleKey`.
    #[serde(alias = "module_key")]
    module_key: Option<String>,
    uri: Option<String>,
    color: Option<String>,
    title: Option<String>,
    position: Option<Position>,
    #[serde(default)]
    ignore_uri_and_module_key_validation: bool,
}

#[derive(Deserialize)]
pub struct Position {
    row: Option<i64>,
    column: Option<i64>,
}

fn check_color(color: &str) -> ApiResult<()> {
    if COLORS.contains(&color) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "The color '{color}' is not valid. Use one of: {}.",
            COLORS.join(", ")
        )))
    }
}

fn check_column(dashboard: &Dashboard, column: i64) -> ApiResult<()> {
    if (0..dashboard.columns()).contains(&column) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "Cannot add gadget to column {column}. The dashboard has {} columns.",
            dashboard.columns()
        )))
    }
}

/// `POST /dashboard/{id}/gadget`.
pub async fn add_gadget(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<GadgetRequest>,
) -> ApiResult<Json<Value>> {
    let dashboard = find_editable(&state, &user, &id).await?;
    let known = request
        .module_key
        .as_deref()
        .and_then(|key| GADGETS.iter().find(|(k, _)| *k == key));
    match (&request.module_key, &request.uri) {
        (Some(_), Some(_)) => {
            return Err(ApiError::bad_request(
                "Specify either the module key or the URI of the gadget, not both.",
            ));
        }
        (None, None) => {
            return Err(ApiError::bad_request(
                "Specify the module key or the URI of the gadget.",
            ));
        }
        _ if known.is_none() && !request.ignore_uri_and_module_key_validation => {
            return Err(ApiError::bad_request(
                "The gadget is not available. Set ignoreUriAndModuleKeyValidation to add it anyway.",
            ));
        }
        _ => {}
    }
    let color = request.color.as_deref().unwrap_or("blue");
    check_color(color)?;
    let column = request
        .position
        .as_ref()
        .and_then(|p| p.column)
        .unwrap_or(0);
    check_column(&dashboard, column)?;
    let title = request
        .title
        .clone()
        .or_else(|| known.map(|(_, title)| title.to_string()))
        .unwrap_or_default();
    let gadget = dashboards::add_gadget(
        &state.db,
        dashboard.id,
        NewGadget {
            module_key: request.module_key.as_deref(),
            uri: request.uri.as_deref(),
            title: &title,
            color,
            col: column,
            row: request.position.as_ref().and_then(|p| p.row),
        },
    )
    .await?;
    Ok(Json(gadget_json(&gadget)))
}

async fn find_gadget(
    state: &AppState,
    dashboard: &Dashboard,
    gadget_id: &str,
) -> ApiResult<Gadget> {
    let missing = || {
        ApiError::not_found(format!(
            "The dashboard gadget with id '{gadget_id}' does not exist on the dashboard."
        ))
    };
    let number: i64 = gadget_id.parse().map_err(|_| missing())?;
    dashboards::gadget(&state.db, dashboard.id, number)
        .await?
        .ok_or_else(missing)
}

/// `PUT /dashboard/{id}/gadget/{gadgetId}`: title, color, and position.
pub async fn update_gadget(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, gadget_id)): Path<(String, String)>,
    Json(request): Json<GadgetRequest>,
) -> ApiResult<StatusCode> {
    let dashboard = find_editable(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &gadget_id).await?;
    let color = request.color.as_deref().unwrap_or(&gadget.color);
    check_color(color)?;
    let (col, row) = match &request.position {
        Some(position) => {
            let col = position.column.unwrap_or(gadget.col);
            check_column(&dashboard, col)?;
            (col, position.row.unwrap_or(0))
        }
        None => (gadget.col, gadget.row),
    };
    let title = request.title.as_deref().unwrap_or(&gadget.title);
    dashboards::update_gadget(&state.db, &gadget, title, color, col, row).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /dashboard/{id}/gadget/{gadgetId}`.
pub async fn delete_gadget(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, gadget_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let dashboard = find_editable(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &gadget_id).await?;
    dashboards::delete_gadget(&state.db, &gadget).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /dashboard/{id}/items/{itemId}/properties`.
pub async fn property_keys(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id, item)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let dashboard = find_visible(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &item).await?;
    let ctx = Ctx::new(&state, version).await?;
    let keys: Vec<Value> = dashboards::property_keys(&state.db, gadget.id)
        .await?
        .into_iter()
        .map(|key| {
            let path = format!("/dashboard/{id}/items/{item}/properties/{key}");
            json!({ "key": key, "self": ctx.api_url(&path) })
        })
        .collect();
    Ok(Json(json!({ "keys": keys })))
}

fn property_missing(key: &str) -> ApiError {
    ApiError::not_found(format!("The property with key '{key}' does not exist."))
}

/// `GET /dashboard/{id}/items/{itemId}/properties/{key}`.
pub async fn property(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, item, key)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let dashboard = find_visible(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &item).await?;
    let text = dashboards::property(&state.db, gadget.id, &key)
        .await?
        .ok_or_else(|| property_missing(&key))?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(Json(json!({ "key": key, "value": value })))
}

/// `PUT /dashboard/{id}/items/{itemId}/properties/{key}`: 201 when new, 200 when replaced.
pub async fn set_property(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, item, key)): Path<(String, String, String)>,
    Json(value): Json<Value>,
) -> ApiResult<Response> {
    let dashboard = find_editable(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &item).await?;
    if key.is_empty() || key.len() > 255 {
        return Err(ApiError::bad_request(
            "The property key must be between 1 and 255 characters.",
        ));
    }
    let created = dashboards::set_property(&state.db, gadget.id, &key, &value.to_string()).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(status.into_response())
}

/// `DELETE /dashboard/{id}/items/{itemId}/properties/{key}`.
pub async fn delete_property(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, item, key)): Path<(String, String, String)>,
) -> ApiResult<StatusCode> {
    let dashboard = find_editable(&state, &user, &id).await?;
    let gadget = find_gadget(&state, &dashboard, &item).await?;
    if !dashboards::delete_property(&state.db, gadget.id, &key).await? {
        return Err(property_missing(&key));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    automatic_refresh_ms: Option<i64>,
}

/// `PUT /rest/internal/latest/dashboards/{id}/automatic-refresh-ms`, which the Python client
/// uses to set how often a dashboard reloads.
pub async fn set_refresh(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<RefreshRequest>,
) -> ApiResult<StatusCode> {
    let mut dashboard = find_editable(&state, &user, &id).await?;
    dashboard.refresh_ms = request.automatic_refresh_ms.filter(|ms| *ms > 0);
    dashboards::update(&state.db, &dashboard).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT` and `DELETE /rest/tessera/1.0/dashboard/{id}/favourite`: Tessera's way to star a
/// dashboard, which Jira's public API lacks.
pub async fn add_favourite(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let dashboard = find_visible(&state, &user, &id).await?;
    dashboards::set_favourite(&state.db, dashboard.id, user.id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_favourite(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let dashboard = find_visible(&state, &user, &id).await?;
    dashboards::set_favourite(&state.db, dashboard.id, user.id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LayoutRequest {
    layout: String,
}

/// `PUT /rest/tessera/1.0/dashboard/{id}/layout`: the column layout. Gadgets in columns that
/// go away move to the end of the last column.
pub async fn set_layout(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<LayoutRequest>,
) -> ApiResult<StatusCode> {
    let mut dashboard = find_editable(&state, &user, &id).await?;
    if !LAYOUTS.contains(&request.layout.as_str()) {
        return Err(ApiError::bad_request(format!(
            "The layout '{}' is not valid. Use one of: {}.",
            request.layout,
            LAYOUTS.join(", ")
        )));
    }
    dashboard.layout = request.layout;
    dashboards::update(&state.db, &dashboard).await?;
    dashboards::fit_columns(&state.db, dashboard.id, dashboard.columns()).await?;
    Ok(StatusCode::NO_CONTENT)
}
