//! Epics: read and update an epic, list and move its issues, and rank epics.

use crate::api::extract::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tessera_jql::ast::Operator;

use super::boards::without_epic;
use super::{
    IssueQuery, MoveRequest, clause, epic_json, extra_clause, is_epic, issue_page, rank_relative,
    ranked, requested_issues, set_epic, value,
};
use crate::AppState;
use crate::api::ApiVersion;
use crate::api::issues::{IssueRequest, find_issue, save_edit};
use crate::api::render::Ctx;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::epics::{self as epic_store, EpicDetails};
use crate::store::issues::{self, Issue};
use crate::store::meta::Catalog;

/// Finds an issue and checks that it is an epic.
async fn find_epic(state: &AppState, id_or_key: &str) -> ApiResult<Issue> {
    let issue = find_issue(state, id_or_key).await?;
    let catalog = Catalog::load(&state.db).await?;
    if !is_epic(&catalog, &issue) {
        return Err(ApiError::not_found(format!(
            "The issue {id_or_key} is not an epic."
        )));
    }
    Ok(issue)
}

pub async fn get(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let epic = find_epic(&state, &id_or_key).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(epic_json(&ctx, &epic).await?))
}

/// The Jira epic colors.
const COLORS: std::ops::RangeInclusive<u32> = 1..=14;

/// Partially updates an epic: `name`, `summary`, `color.key`, and `done`.
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let epic = find_epic(&state, &id_or_key).await?;
    let mut details = epic_store::details(&state.db, epic.id).await?;
    if let Some(name) = body.get("name").and_then(Value::as_str) {
        details.name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
    }
    if let Some(color) = body
        .get("color")
        .and_then(|c| c.get("key"))
        .and_then(Value::as_str)
    {
        let valid = color
            .strip_prefix("color_")
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| COLORS.contains(&n));
        if !valid {
            return Err(ApiError::field(
                "color",
                "The color must be color_1 to color_14.",
            ));
        }
        details.color_key = Some(color.to_string());
    }
    if let Some(done) = body.get("done").and_then(Value::as_bool) {
        details.done = Some(done);
    }
    epic_store::save(&state.db, epic.id, &EpicDetails { ..details }).await?;
    if let Some(summary) = body.get("summary").and_then(Value::as_str) {
        let fields = Map::from_iter([("summary".to_string(), json!(summary))]);
        let mut ctx = Ctx::new(&state, ApiVersion::V2).await?;
        save_edit(
            &mut ctx,
            &epic,
            user.id,
            &IssueRequest {
                fields,
                update: Map::new(),
            },
        )
        .await?;
    }
    let updated = find_epic(&state, &id_or_key).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(epic_json(&ctx, &updated).await?))
}

pub async fn issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let epic = find_epic(&state, &id_or_key).await?;
    let query = ranked(vec![
        Some(clause("parent", Operator::Equals, value(epic.key))),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

/// Puts issues in the epic. An issue is in one epic at a time, so this moves them.
pub async fn move_issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    let epic = find_epic(&state, &id_or_key).await?;
    for issue in requested_issues(&state, &request.issues).await? {
        set_epic(&state, &user, &issue, Some(&epic)).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn issues_without_epic(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let query = ranked(vec![
        Some(without_epic()),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

/// Takes issues out of their epics.
pub async fn remove_from_epic(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    let catalog = Catalog::load(&state.db).await?;
    for issue in requested_issues(&state, &request.issues).await? {
        let in_epic = match issue.parent_id {
            Some(parent) => issues::by_id(&state.db, parent)
                .await?
                .is_some_and(|p| is_epic(&catalog, &p)),
            None => false,
        };
        if in_epic {
            set_epic(&state, &user, &issue, None).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpicRankRequest {
    rank_before_epic: Option<String>,
    rank_after_epic: Option<String>,
}

/// Ranks an epic before or after another epic.
pub async fn rank(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<EpicRankRequest>,
) -> ApiResult<StatusCode> {
    let epic = find_epic(&state, &id_or_key).await?;
    for anchor in [&request.rank_before_epic, &request.rank_after_epic]
        .into_iter()
        .flatten()
    {
        find_epic(&state, anchor).await?;
    }
    rank_relative(
        &state,
        std::slice::from_ref(&epic),
        request.rank_before_epic.as_deref(),
        request.rank_after_epic.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
