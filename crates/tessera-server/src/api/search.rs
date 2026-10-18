//! JQL search: the older `/search` (page by `startAt`) and the newer `/search/jql`
//! (page by `nextPageToken`, and only issue ids unless `fields` asks for more).

use super::extract::Json;
use axum::Extension;
use axum::extract::{Query, RawQuery, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::sqlite::SqliteArguments;
use sqlx::{Arguments, SqlitePool};

use super::ApiVersion;
use super::issue_fields::CustomCatalog;
use super::issues::{IssueView, issue_json, query_list};
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::jql::{self, Bind, SearchContext};
use crate::store::users::{self, User};
use crate::store::{components, issues, links, projects, sprints, versions};
use crate::time::now_ms;

/// The most issues one page may hold.
const MAX_PAGE: i64 = 5000;

/// Parses JQL, as Jira reports parse errors.
pub fn parse_jql(jql_text: &str) -> ApiResult<tessera_jql::ast::Query> {
    tessera_jql::parse(jql_text).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Error in the JQL Query: {}", e.message),
        )
    })
}

/// Runs a JQL query and returns one page of issue ids plus the total match count.
pub async fn run_jql(
    db: &SqlitePool,
    user: Option<&User>,
    jql_text: &str,
    start_at: i64,
    max_results: i64,
) -> ApiResult<(Vec<i64>, i64)> {
    run_query(db, user, &parse_jql(jql_text)?, start_at, max_results).await
}

/// Runs a parsed query. Agile views build queries by combining parsed parts.
pub async fn run_query(
    db: &SqlitePool,
    user: Option<&User>,
    query: &tessera_jql::ast::Query,
    start_at: i64,
    max_results: i64,
) -> ApiResult<(Vec<i64>, i64)> {
    let catalog = crate::store::meta::Catalog::load(db).await?;
    let custom = CustomCatalog::load(db).await?;
    let all_projects = projects::list(db).await?;
    let all_users = users::all(db).await?;
    let all_components = components::all(db).await?;
    let all_versions = versions::all(db).await?;
    let link_types = links::types(db).await?;
    let all_sprints = sprints::all(db).await?;
    let visible_filters = super::filters::visible(db, user).await?;
    let security_levels = crate::store::security::all_levels(db).await?;
    let ctx = SearchContext {
        catalog: &catalog,
        custom: &custom,
        projects: &all_projects,
        users: &all_users,
        components: &all_components,
        versions: &all_versions,
        link_types: &link_types,
        sprints: &all_sprints,
        filters: &visible_filters,
        security_levels: &security_levels,
        current_user: user.map(|u| u.id),
        now: now_ms(),
    };
    let mut compiled =
        jql::compile(query, &ctx).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.0))?;
    // Issues in projects the user cannot browse are never found.
    let browsable = match user {
        Some(user) => crate::api::access::browsable_projects(db, user).await?,
        None => Some(Vec::new()),
    };
    if let Some(ids) = browsable {
        let marks = vec!["?"; ids.len()].join(", ");
        compiled.filter = if ids.is_empty() {
            format!("({}) AND 0 = 1", compiled.filter)
        } else {
            format!("({}) AND i.project_id IN ({marks})", compiled.filter)
        };
        compiled.binds.extend(ids.into_iter().map(Bind::Int));
    }
    // Nor are issues at security levels the user is not a member of.
    let security = match user {
        Some(user) => crate::api::access::security_filter(db, user).await?,
        None => Some(("i.security_level_id IS NULL".to_string(), Vec::new())),
    };
    if let Some((condition, binds)) = security {
        compiled.filter = format!("({}) AND {condition}", compiled.filter);
        compiled.binds.extend(binds.into_iter().map(Bind::Int));
    }

    let arguments = |extra: &[i64]| {
        let mut args = SqliteArguments::default();
        for bind in &compiled.binds {
            let added = match bind {
                Bind::Int(value) => args.add(*value),
                Bind::Real(value) => args.add(*value),
                Bind::Text(value) => args.add(value.clone()),
            };
            added.expect("SQLite accepts these types");
        }
        for value in extra {
            args.add(*value).expect("SQLite accepts integers");
        }
        args
    };
    // Safe: the JQL compiler emits only its own SQL text. Every user-supplied value is bound
    // through `arguments`, never written into the string.
    let ids: Vec<i64> = sqlx::query_scalar_with(
        sqlx::AssertSqlSafe(format!(
            "SELECT i.id FROM issues i WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
            compiled.filter, compiled.order_by
        )),
        arguments(&[max_results, start_at]),
    )
    .fetch_all(db)
    .await?;
    let total: i64 = sqlx::query_scalar_with(
        sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM issues i WHERE {}",
            compiled.filter
        )),
        arguments(&[]),
    )
    .fetch_one(db)
    .await?;
    Ok((ids, total))
}

async fn render_page(
    ctx: &mut Ctx,
    ids: &[i64],
    view: &IssueView,
    ids_only: bool,
) -> ApiResult<Vec<Value>> {
    let custom = CustomCatalog::load(&ctx.state.db).await?;
    let found = issues::by_ids(&ctx.state.db, ids).await?;
    let mut rendered = Vec::with_capacity(found.len());
    for issue in &found {
        rendered.push(if ids_only {
            json!({ "id": issue.id.to_string() })
        } else {
            issue_json(ctx, &custom, issue, view).await?
        });
    }
    Ok(rendered)
}

/// Field id to display name, for `expand=names` at the top level of a search result.
async fn names(db: &SqlitePool) -> ApiResult<Value> {
    let custom = CustomCatalog::load(db).await?;
    let mut names = serde_json::Map::new();
    for field in super::fields::SYSTEM_FIELDS {
        names.insert(field.id.into(), json!(field.name));
    }
    for field in &custom.fields {
        names.insert(super::fields::custom_field_key(field.id), json!(field.name));
    }
    Ok(Value::Object(names))
}

/// A list parameter that clients send as a JSON array or a comma-separated string.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ListParam {
    #[default]
    None,
    List(Vec<String>),
    Text(String),
}

impl ListParam {
    fn items(&self) -> Vec<String> {
        match self {
            Self::None => Vec::new(),
            Self::List(items) => items.clone(),
            Self::Text(text) => text
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    #[serde(default)]
    jql: String,
    #[serde(default)]
    start_at: i64,
    max_results: Option<i64>,
    #[serde(default)]
    fields: ListParam,
    #[serde(default)]
    expand: ListParam,
    next_page_token: Option<String>,
}

/// The older search response: `startAt`, `maxResults`, `total`, and full issues.
async fn legacy(
    state: &AppState,
    version: ApiVersion,
    user: &User,
    request: SearchRequest,
) -> ApiResult<Json<Value>> {
    let start_at = request.start_at.max(0);
    let max_results = request.max_results.unwrap_or(50).clamp(0, MAX_PAGE);
    let (ids, total) = run_jql(&state.db, Some(user), &request.jql, start_at, max_results).await?;
    let expand = request.expand.items();
    let view = IssueView::new(&request.fields.items(), &expand);
    let mut ctx = Ctx::new(state, version).await?.viewed_by(user.id);
    let issues = render_page(&mut ctx, &ids, &view, false).await?;
    let mut response = json!({
        "expand": "schema,names",
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "issues": issues,
    });
    if expand.iter().any(|e| e == "names") {
        response["names"] = names(&state.db).await?;
    }
    Ok(Json(response))
}

/// The newer search response: issues, `isLast`, and `nextPageToken` when more pages exist.
async fn enhanced(
    state: &AppState,
    version: ApiVersion,
    user: &User,
    request: SearchRequest,
) -> ApiResult<Json<Value>> {
    // The token is the offset of the next page. Clients treat it as opaque.
    let start_at = match request.next_page_token.as_deref() {
        Some(token) => token
            .parse::<i64>()
            .map_err(|_| ApiError::bad_request("The nextPageToken is not valid."))?,
        None => 0,
    };
    let max_results = request.max_results.unwrap_or(50).clamp(0, MAX_PAGE);
    let (ids, total) = run_jql(&state.db, Some(user), &request.jql, start_at, max_results).await?;
    let fields = request.fields.items();
    let expand = request.expand.items();
    let ids_only = fields.is_empty() || fields.iter().all(|f| f == "id");
    let view = IssueView::new(&fields, &expand);
    let mut ctx = Ctx::new(state, version).await?.viewed_by(user.id);
    let issues = render_page(&mut ctx, &ids, &view, ids_only).await?;
    let next = start_at + ids.len() as i64;
    let mut response = json!({ "issues": issues, "isLast": next >= total });
    if next < total {
        response["nextPageToken"] = json!(next.to_string());
    }
    if expand.iter().any(|e| e == "names") {
        response["names"] = names(&state.db).await?;
    }
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    jql: String,
    #[serde(default)]
    start_at: i64,
    max_results: Option<i64>,
    next_page_token: Option<String>,
}

fn from_query(query: SearchQuery, raw: Option<&str>) -> SearchRequest {
    SearchRequest {
        jql: query.jql,
        start_at: query.start_at,
        max_results: query.max_results,
        fields: ListParam::List(query_list(raw, "fields")),
        expand: ListParam::List(query_list(raw, "expand")),
        next_page_token: query.next_page_token,
    }
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    legacy(&state, version, &user, from_query(query, raw.as_deref())).await
}

pub async fn post(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SearchRequest>,
) -> ApiResult<Json<Value>> {
    legacy(&state, version, &user, request).await
}

pub async fn jql_get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    enhanced(&state, version, &user, from_query(query, raw.as_deref())).await
}

pub async fn jql_post(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<SearchRequest>,
) -> ApiResult<Json<Value>> {
    enhanced(&state, version, &user, request).await
}
