//! Sprints: create, read, update (start and complete), delete, move issues, swap, properties.
//!
//! Lifecycle rules, as in Jira:
//! - A sprint starts as `future`. Setting `state` to `active` starts it; it needs start and end
//!   dates. Only one sprint per board can be active.
//! - Setting `state` to `closed` completes an active sprint and sets `completeDate`. Its
//!   unfinished issues return to the backlog; the sprint stays in their history.
//! - A closed sprint only accepts changes to its name and goal.
//! - Deleting a sprint moves its open issues to the backlog.

use crate::api::extract::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use tessera_jql::ast::Operator;

use super::boards::find_board;
use super::{
    IssueQuery, MoveRequest, agile_url, clause, extra_clause, issue_page, properties,
    rank_relative, ranked, requested_issues, set_sprint, sprint_json, value,
};
use crate::AppState;
use crate::api::ApiVersion;
use crate::api::access::require_project;
use crate::api::render::Ctx;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::properties::Entity;
use crate::store::sprints::{self, Sprint};
use crate::store::users::User;
use crate::store::{boards, projects};
use crate::time::{now_ms, parse_jira_datetime};

async fn find(state: &AppState, id: &str) -> ApiResult<Sprint> {
    let not_found = || ApiError::not_found(format!("Sprint with id {id} does not exist."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    sprints::by_id(&state.db, id).await?.ok_or_else(not_found)
}

/// Changing sprints needs the Manage Sprints permission in the project of the sprint's board.
async fn check_manage(state: &AppState, user: &User, board_id: Option<i64>) -> ApiResult<()> {
    let Some(board) = board_id else { return Ok(()) };
    let Some(project_id) = boards::by_id(&state.db, board)
        .await?
        .and_then(|b| b.project_id)
    else {
        return Ok(());
    };
    if let Some(project) = projects::by_id(&state.db, project_id).await? {
        require_project(state, user, &project, "MANAGE_SPRINTS_PERMISSION").await?;
    }
    Ok(())
}

/// A sprint date from a request: `None` when absent, `Some(None)` for null.
fn date(body: &Value, key: &str) -> ApiResult<Option<Option<i64>>> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) => parse_jira_datetime(text)
            .map(|ms| Some(Some(ms)))
            .ok_or_else(|| ApiError::field(key, "The date is not valid.")),
        Some(_) => Err(ApiError::field(key, "The date is not valid.")),
    }
}

fn check_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::field(
            "name",
            "The sprint name must not be empty.",
        ));
    }
    if name.chars().count() > 255 {
        return Err(ApiError::field(
            "name",
            "The sprint name must be 255 characters or less.",
        ));
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    check_name(&name)?;
    let board_id = body
        .get("originBoardId")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::field("originBoardId", "The board id is required."))?;
    let board = find_board(&state, &board_id.to_string()).await?;
    check_manage(&state, &user, Some(board.id)).await?;
    if board.board_type != "scrum" {
        return Err(ApiError::bad_request("The board does not support sprints"));
    }
    let start = date(&body, "startDate")?.flatten();
    let end = date(&body, "endDate")?.flatten();
    check_dates(start, end)?;
    let goal = body.get("goal").and_then(Value::as_str).unwrap_or("");
    let sprint = sprints::create(&state.db, &name, goal, start, end, board.id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok((StatusCode::CREATED, Json(sprint_json(&ctx, &sprint))))
}

fn check_dates(start: Option<i64>, end: Option<i64>) -> ApiResult<()> {
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(ApiError::field(
            "endDate",
            "The end date must be after the start date.",
        ));
    }
    Ok(())
}

pub async fn get(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let sprint = find(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(sprint_json(&ctx, &sprint)))
}

/// Applies an update. `full` is a PUT: absent fields become null.
async fn apply(
    state: &AppState,
    mut sprint: Sprint,
    body: &Value,
    full: bool,
) -> ApiResult<Sprint> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    if let Some(name) = text("name") {
        check_name(&name)?;
        sprint.name = name.trim().to_string();
    }
    match text("goal") {
        Some(goal) => sprint.goal = goal,
        None if full => sprint.goal = String::new(),
        None => {}
    }
    if sprint.state == "closed" {
        // Closed sprints only change their name and goal; other fields are ignored.
        sprints::save(&state.db, &sprint).await?;
        return Ok(sprint);
    }
    match date(body, "startDate")? {
        Some(start) => sprint.start_date = start,
        None if full => sprint.start_date = None,
        None => {}
    }
    match date(body, "endDate")? {
        Some(end) => sprint.end_date = end,
        None if full => sprint.end_date = None,
        None => {}
    }
    check_dates(sprint.start_date, sprint.end_date)?;
    if let Some(new_state) = text("state")
        && new_state != sprint.state
    {
        match (sprint.state.as_str(), new_state.as_str()) {
            ("future", "active") => {
                if sprint.start_date.is_none() || sprint.end_date.is_none() {
                    return Err(ApiError::bad_request(
                        "A sprint needs a start and end date to be started.",
                    ));
                }
                let busy = match sprint.origin_board_id {
                    Some(board) => sprints::of_board(&state.db, board)
                        .await?
                        .iter()
                        .any(|s| s.state == "active" && s.id != sprint.id),
                    None => false,
                };
                if busy {
                    return Err(ApiError::bad_request(
                        "A sprint is already active on this board.",
                    ));
                }
                sprint.state = "active".into();
            }
            ("active", "closed") => {
                sprint.state = "closed".into();
                sprint.complete_date = Some(now_ms());
            }
            (from, to) => {
                return Err(ApiError::bad_request(format!(
                    "A sprint cannot change from '{from}' to '{to}'."
                )));
            }
        }
    }
    sprints::save(&state.db, &sprint).await?;
    Ok(sprint)
}

/// `PUT /sprint/{id}`: a full update.
pub async fn replace(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let found = find(&state, &id).await?;
    check_manage(&state, &user, found.origin_board_id).await?;
    let sprint = apply(&state, found, &body, true).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(sprint_json(&ctx, &sprint)))
}

/// `POST /sprint/{id}`: a partial update.
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let found = find(&state, &id).await?;
    check_manage(&state, &user, found.origin_board_id).await?;
    let sprint = apply(&state, found, &body, false).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(sprint_json(&ctx, &sprint)))
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let sprint = find(&state, &id).await?;
    check_manage(&state, &user, sprint.origin_board_id).await?;
    if sprint.state == "closed" {
        return Err(ApiError::bad_request("Closed sprints cannot be deleted."));
    }
    sprints::delete(&state.db, sprint.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let sprint = find(&state, &id).await?;
    let query = ranked(vec![
        Some(clause(
            "sprint",
            Operator::Equals,
            value(sprint.id.to_string()),
        )),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

/// Moves issues into the sprint, optionally ranking them before or after another issue.
pub async fn move_issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    let sprint = find(&state, &id).await?;
    if !sprint.is_open() {
        return Err(ApiError::bad_request(
            "Issues can only be moved to open or active sprints.",
        ));
    }
    let issues = requested_issues(&state, &user, &request.issues).await?;
    for issue in &issues {
        set_sprint(&state, &user, issue, Some(sprint.id)).await?;
    }
    rank_relative(
        &state,
        &user,
        &issues,
        request.rank_before_issue.as_deref(),
        request.rank_after_issue.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapRequest {
    sprint_to_swap_with: i64,
}

/// Swaps the board positions of two sprints that are not closed.
pub async fn swap(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<SwapRequest>,
) -> ApiResult<StatusCode> {
    let first = find(&state, &id).await?;
    check_manage(&state, &user, first.origin_board_id).await?;
    let second = find(&state, &request.sprint_to_swap_with.to_string()).await?;
    if !first.is_open() || !second.is_open() {
        return Err(ApiError::bad_request("Closed sprints cannot be swapped."));
    }
    sprints::swap(&state.db, &first, &second).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn property_keys(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let sprint = find(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    properties::keys(
        &state,
        Entity::Sprint,
        sprint.id,
        &agile_url(&ctx, &format!("/sprint/{}", sprint.id)),
    )
    .await
}

pub async fn get_property(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let sprint = find(&state, &id).await?;
    properties::get(&state, Entity::Sprint, sprint.id, &key).await
}

pub async fn set_property(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, key)): Path<(String, String)>,
    Json(value): Json<Value>,
) -> ApiResult<StatusCode> {
    let sprint = find(&state, &id).await?;
    properties::set(&state, Entity::Sprint, sprint.id, &key, &value).await
}

pub async fn delete_property(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let sprint = find(&state, &id).await?;
    properties::delete(&state, Entity::Sprint, sprint.id, &key).await
}
