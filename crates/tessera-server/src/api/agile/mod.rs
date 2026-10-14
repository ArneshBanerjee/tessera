//! The Jira Software (Agile) REST API under `/rest/agile/1.0`: boards, sprints, the backlog,
//! epics, and ranking.
//!
//! Agile responses use the v2 issue representation (rich text as plain strings), as in Jira.
//! Board, backlog, sprint, and epic views are built by combining parsed JQL, never by gluing
//! JQL strings together.

mod boards;
mod epics;
mod issues;
mod sprints;

pub use boards::create_board;

use crate::api::access::find_issue_as;
use axum::routing::{get, post, put};
use axum::{Extension, Router};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tessera_jql::ast::{Clause, Direction, Field, Operand, Operator, OrderBy, Query, Terminal};

use super::ApiVersion;
use super::issue_fields::CustomCatalog;
use super::issues::{IssueRequest, IssueView, issue_json, save_edit};
use super::render::Ctx;
use super::search::{parse_jql, run_query};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::store::issues::{self as issue_store, Issue};
use crate::store::meta::Catalog;
use crate::store::sprints::Sprint;
use crate::store::users::User;
use crate::store::{epics as epic_store, ranks, sprints as sprint_store};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/board", get(boards::list).post(boards::create))
        .route("/board/filter/{filter_id}", get(boards::by_filter))
        .route("/board/{id}", get(boards::get).delete(boards::delete))
        .route("/board/{id}/configuration", get(boards::configuration))
        .route("/board/{id}/issue", get(boards::issues))
        .route("/board/{id}/backlog", get(boards::backlog))
        .route("/board/{id}/sprint", get(boards::sprints))
        .route(
            "/board/{id}/sprint/{sprint_id}/issue",
            get(boards::sprint_issues),
        )
        .route("/board/{id}/epic", get(boards::epics))
        .route(
            "/board/{id}/epic/none/issue",
            get(boards::issues_without_epic),
        )
        .route("/board/{id}/epic/{epic_id}/issue", get(boards::epic_issues))
        .route("/board/{id}/project", get(boards::projects))
        .route("/board/{id}/version", get(boards::versions))
        .route("/board/{id}/quickfilter", get(boards::quick_filters))
        .route("/board/{id}/features", get(boards::features))
        .route("/board/{id}/properties", get(boards::property_keys))
        .route(
            "/board/{id}/properties/{key}",
            get(boards::get_property)
                .put(boards::set_property)
                .delete(boards::delete_property),
        )
        .route("/sprint", post(sprints::create))
        .route(
            "/sprint/{id}",
            get(sprints::get)
                .put(sprints::replace)
                .post(sprints::update)
                .delete(sprints::delete),
        )
        .route(
            "/sprint/{id}/issue",
            get(sprints::issues).post(sprints::move_issues),
        )
        .route("/sprint/{id}/swap", post(sprints::swap))
        .route("/sprint/{id}/properties", get(sprints::property_keys))
        .route(
            "/sprint/{id}/properties/{key}",
            get(sprints::get_property)
                .put(sprints::set_property)
                .delete(sprints::delete_property),
        )
        .route(
            "/epic/none/issue",
            get(epics::issues_without_epic).post(epics::remove_from_epic),
        )
        .route("/epic/{id_or_key}", get(epics::get).post(epics::update))
        .route(
            "/epic/{id_or_key}/issue",
            get(epics::issues).post(epics::move_issues),
        )
        .route("/epic/{id_or_key}/rank", put(epics::rank))
        .route("/issue/rank", put(issues::rank))
        .route("/issue/{id_or_key}", get(issues::get))
        .route(
            "/issue/{id_or_key}/estimation",
            get(issues::get_estimation).put(issues::set_estimation),
        )
        .route("/backlog/issue", post(issues::to_backlog))
        .route("/backlog/{board_id}/issue", post(issues::to_board_backlog))
        // Agile responses use the v2 issue representation.
        .layer(Extension(ApiVersion::V2))
}

/// An absolute URL in the Agile API.
fn agile_url(ctx: &Ctx, path: &str) -> String {
    format!("{}/rest/agile/1.0{path}", ctx.state.base_url)
}

pub fn sprint_json(ctx: &Ctx, sprint: &Sprint) -> Value {
    let mut value = json!({
        "id": sprint.id,
        "self": agile_url(ctx, &format!("/sprint/{}", sprint.id)),
        "state": sprint.state,
        "name": sprint.name,
        "goal": sprint.goal,
    });
    let date = |ms: Option<i64>| ms.map(crate::time::format);
    if let Some(start) = date(sprint.start_date) {
        value["startDate"] = json!(start);
    }
    if let Some(end) = date(sprint.end_date) {
        value["endDate"] = json!(end);
    }
    if let Some(complete) = date(sprint.complete_date) {
        value["completeDate"] = json!(complete);
    }
    if let Some(board) = sprint.origin_board_id {
        value["originBoardId"] = json!(board);
        value["boardId"] = json!(board);
    }
    value
}

/// The Agile view of an epic.
async fn epic_json(ctx: &Ctx, epic: &Issue) -> ApiResult<Value> {
    let details = epic_store::details(&ctx.state.db, epic.id).await?;
    let done = details
        .done
        .unwrap_or_else(|| ctx.catalog.is_done(epic.status_id));
    Ok(json!({
        "id": epic.id,
        "key": epic.key,
        "self": agile_url(ctx, &format!("/epic/{}", epic.id)),
        "name": details.name.unwrap_or_else(|| epic.summary.clone()),
        "summary": epic.summary,
        "color": { "key": details.color_key.unwrap_or_else(|| "color_1".into()) },
        "done": done,
    }))
}

fn is_epic(catalog: &Catalog, issue: &Issue) -> bool {
    catalog
        .issue_type(issue.issue_type_id)
        .is_some_and(|t| t.hierarchy_level == 1)
}

/// An issue with the Agile fields: `sprint`, `closedSprints`, `epic`, and `flagged`.
async fn agile_issue_json(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    issue: &Issue,
) -> ApiResult<Value> {
    let mut value = issue_json(ctx, custom, issue, &IssueView::default()).await?;
    let db = ctx.state.db.clone();
    let all = sprint_store::of_issue(&db, issue.id).await?;
    let open = all
        .iter()
        .find(|s| s.is_open())
        .map(|s| sprint_json(ctx, s));
    let closed: Vec<Value> = all
        .iter()
        .filter(|s| !s.is_open())
        .map(|s| sprint_json(ctx, s))
        .collect();
    let epic = match issue.parent_id {
        Some(parent_id) => match issue_store::by_id(&db, parent_id).await? {
            Some(parent) if is_epic(&ctx.catalog, &parent) => epic_json(ctx, &parent).await?,
            _ => Value::Null,
        },
        None => Value::Null,
    };
    let fields = value["fields"].as_object_mut().expect("issues have fields");
    fields.insert("sprint".into(), open.unwrap_or(Value::Null));
    fields.insert("closedSprints".into(), Value::Array(closed));
    fields.insert("epic".into(), epic);
    fields.insert("flagged".into(), json!(false));
    Ok(value)
}

/// `startAt`, `maxResults`, and `jql` for Agile issue lists.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IssueQuery {
    #[serde(default)]
    start_at: i64,
    max_results: Option<i64>,
    jql: Option<String>,
}

/// A JQL clause such as `sprint in openSprints()`, built directly.
fn clause(field: &str, operator: Operator, operand: Operand) -> Clause {
    Clause::Terminal(Terminal {
        field: Field::Name(field.into()),
        operator,
        operand: Some(operand),
        predicates: Vec::new(),
    })
}

fn function(name: &str, args: &[&str]) -> Operand {
    Operand::Function {
        name: name.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
    }
}

fn value(text: impl Into<String>) -> Operand {
    Operand::Value {
        text: text.into(),
        quoted: true,
    }
}

/// Combines parts with AND and orders by rank, which is how boards order issues.
fn ranked(parts: Vec<Option<Clause>>) -> Query {
    let parts: Vec<Clause> = parts.into_iter().flatten().collect();
    let filter = match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => Some(Clause::And(parts)),
    };
    Query {
        filter,
        order_by: vec![OrderBy {
            field: Field::Name("rank".into()),
            direction: Some(Direction::Asc),
        }],
    }
}

/// The optional `jql` parameter as a clause.
fn extra_clause(jql: Option<&str>) -> ApiResult<Option<Clause>> {
    match jql.map(str::trim).filter(|j| !j.is_empty()) {
        Some(jql) => Ok(parse_jql(jql)?.filter),
        None => Ok(None),
    }
}

/// Runs a query and renders the page in the Agile list shape.
async fn issue_page(
    state: &AppState,
    user: &User,
    query: &Query,
    params: &IssueQuery,
) -> ApiResult<Value> {
    let start_at = params.start_at.max(0);
    let max_results = params.max_results.unwrap_or(50).clamp(0, 1000);
    let (ids, total) = run_query(&state.db, Some(user), query, start_at, max_results).await?;
    let mut ctx = Ctx::new(state, ApiVersion::V2).await?.viewed_by(user.id);
    let custom = CustomCatalog::load(&state.db).await?;
    let mut rendered = Vec::with_capacity(ids.len());
    for issue in issue_store::by_ids(&state.db, &ids).await? {
        rendered.push(agile_issue_json(&mut ctx, &custom, &issue).await?);
    }
    Ok(json!({
        "expand": "names,schema",
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
        "issues": rendered,
    }))
}

/// A page of plain JSON values, in the Agile API's paging shape.
fn page(values: Vec<Value>, start_at: i64, max_results: i64) -> Value {
    let total = values.len() as i64;
    let start_at = start_at.max(0);
    let max_results = max_results.clamp(0, 1000);
    let values: Vec<Value> = values
        .into_iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .collect();
    json!({
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageQuery {
    #[serde(default)]
    start_at: i64,
    #[serde(default = "default_page")]
    max_results: i64,
}

fn default_page() -> i64 {
    50
}

/// The issues named in a move or rank request (keys or ids), at most 50, all existing.
async fn requested_issues(state: &AppState, user: &User, keys: &[String]) -> ApiResult<Vec<Issue>> {
    if keys.len() > 50 {
        return Err(ApiError::bad_request(
            "At most 50 issues may be moved or ranked at once.",
        ));
    }
    let mut found = Vec::with_capacity(keys.len());
    for key in keys {
        found.push(find_issue_as(state, user, key, "EDIT_ISSUES").await?);
    }
    Ok(found)
}

/// Puts issues before or after another issue in the global rank.
async fn rank_relative(
    state: &AppState,
    user: &User,
    issues: &[Issue],
    before: Option<&str>,
    after: Option<&str>,
) -> ApiResult<()> {
    let (anchor, before_anchor) = match (before, after) {
        (Some(key), _) => (
            find_issue_as(state, user, key, "BROWSE_PROJECTS").await?,
            true,
        ),
        (None, Some(key)) => (
            find_issue_as(state, user, key, "BROWSE_PROJECTS").await?,
            false,
        ),
        (None, None) => return Ok(()),
    };
    if issues.iter().any(|i| i.id == anchor.id) {
        return Err(ApiError::bad_request(
            "An issue cannot be ranked relative to itself.",
        ));
    }
    let ids: Vec<i64> = issues.iter().map(|i| i.id).collect();
    ranks::place(&state.db, &ids, anchor.id, before_anchor).await?;
    Ok(())
}

/// Moves an issue into a sprint (or out of all open sprints) through a normal edit, so the
/// change appears in the issue history.
async fn set_sprint(
    state: &AppState,
    user: &User,
    issue: &Issue,
    sprint: Option<i64>,
) -> ApiResult<()> {
    let custom = CustomCatalog::load(&state.db).await?;
    let field = custom
        .fields
        .iter()
        .find(|f| f.field_type == "sprint")
        .expect("the Sprint field is created by a migration");
    let mut fields = Map::new();
    fields.insert(super::fields::custom_field_key(field.id), json!(sprint));
    let mut ctx = Ctx::new(state, ApiVersion::V2).await?;
    save_edit(
        &mut ctx,
        issue,
        user.id,
        &IssueRequest {
            fields,
            update: Map::new(),
        },
    )
    .await
    .map(|_| ())
}

/// Sets or clears the parent epic of an issue through a normal edit.
async fn set_epic(
    state: &AppState,
    user: &User,
    issue: &Issue,
    epic: Option<&Issue>,
) -> ApiResult<()> {
    let parent = match epic {
        Some(epic) => json!({ "key": epic.key }),
        None => Value::Null,
    };
    let fields = Map::from_iter([("parent".to_string(), parent)]);
    let mut ctx = Ctx::new(state, ApiVersion::V2).await?;
    save_edit(
        &mut ctx,
        issue,
        user.id,
        &IssueRequest {
            fields,
            update: Map::new(),
        },
    )
    .await
    .map(|_| ())
}

/// `{"issues": [...], "rankBeforeIssue": ..., "rankAfterIssue": ...}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    #[serde(default)]
    issues: Vec<String>,
    rank_before_issue: Option<String>,
    rank_after_issue: Option<String>,
}

/// Properties of boards and sprints share these handlers.
mod properties {
    use crate::api::extract::Json;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::AppState;
    use crate::error::{ApiError, ApiResult};
    use crate::store::properties::{self, Entity};

    pub async fn keys(
        state: &AppState,
        entity: Entity,
        id: i64,
        base: &str,
    ) -> ApiResult<Json<Value>> {
        let keys = properties::keys(&state.db, entity, id).await?;
        let keys: Vec<Value> = keys
            .iter()
            .map(|key| json!({ "self": format!("{base}/properties/{key}"), "key": key }))
            .collect();
        Ok(Json(json!({ "keys": keys })))
    }

    pub async fn get(
        state: &AppState,
        entity: Entity,
        id: i64,
        key: &str,
    ) -> ApiResult<Json<Value>> {
        let value = properties::get(&state.db, entity, id, key)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("The property '{key}' does not exist.")))?;
        let value: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
        Ok(Json(json!({ "key": key, "value": value })))
    }

    pub async fn set(
        state: &AppState,
        entity: Entity,
        id: i64,
        key: &str,
        value: &Value,
    ) -> ApiResult<StatusCode> {
        if key.len() > 255 {
            return Err(ApiError::bad_request("The property key is too long."));
        }
        let created = properties::set(&state.db, entity, id, key, &value.to_string()).await?;
        Ok(if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        })
    }

    pub async fn delete(
        state: &AppState,
        entity: Entity,
        id: i64,
        key: &str,
    ) -> ApiResult<StatusCode> {
        if properties::delete(&state.db, entity, id, key).await? {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(ApiError::not_found(format!(
                "The property '{key}' does not exist."
            )))
        }
    }
}
