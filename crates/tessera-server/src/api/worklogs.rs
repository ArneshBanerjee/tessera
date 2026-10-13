//! Worklogs: `/issue/{idOrKey}/worklog` and `/issue/{idOrKey}/worklog/{id}`.
//!
//! Logging work changes the issue's remaining estimate according to `adjustEstimate`:
//! - `auto` (default): reduce it by the time logged (never below zero); deleting adds it back.
//! - `new`: set it to `newEstimate`.
//! - `manual`: reduce it by `reduceBy` (or increase by `increaseBy` on delete).
//! - `leave`: keep it.

use super::extract::Json;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiVersion;
use super::issue_fields::CustomCatalog;
use super::issues::find_issue;
use super::render::{Ctx, rich_text_input};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::issues::{self, ChangeItem, Issue, IssueFields};
use crate::store::users::User;
use crate::store::worklogs::{self, Worklog};
use crate::time;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AdjustQuery {
    adjust_estimate: Option<String>,
    new_estimate: Option<String>,
    reduce_by: Option<String>,
    increase_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorklogRequest {
    time_spent: Option<String>,
    time_spent_seconds: Option<i64>,
    started: Option<String>,
    comment: Option<Value>,
}

/// Validated worklog input: comment (ADF JSON), start, and seconds.
struct WorklogInput {
    comment: Option<String>,
    started: i64,
    seconds: i64,
}

fn parse_request(request: &WorklogRequest, current: Option<&Worklog>) -> ApiResult<WorklogInput> {
    let seconds = match (&request.time_spent_seconds, &request.time_spent) {
        (Some(seconds), _) => *seconds,
        (None, Some(text)) => time::parse_duration(text).ok_or_else(|| {
            ApiError::field(
                "timeLogged",
                format!("Invalid time duration entered: '{text}'."),
            )
        })?,
        (None, None) => current.map(|w| w.time_spent_seconds).ok_or_else(|| {
            ApiError::field("timeLogged", "You must indicate the time spent working.")
        })?,
    };
    if seconds <= 0 {
        return Err(ApiError::field(
            "timeLogged",
            "Time spent must be greater than zero.",
        ));
    }
    let started = match &request.started {
        Some(text) => time::parse_jira_datetime(text)
            .ok_or_else(|| ApiError::field("started", "The start date and time is not valid."))?,
        None => current.map(|w| w.started).unwrap_or_else(time::now_ms),
    };
    let comment = match &request.comment {
        Some(value) => rich_text_input("comment", value)?,
        None => current.and_then(|w| w.comment.clone()),
    };
    Ok(WorklogInput {
        comment,
        started,
        seconds,
    })
}

fn estimate(text: Option<&str>, name: &str) -> ApiResult<i64> {
    let text =
        text.ok_or_else(|| ApiError::bad_request(format!("The {name} parameter is required.")))?;
    time::parse_duration(text)
        .ok_or_else(|| ApiError::bad_request(format!("The {name} '{text}' is not valid.")))
}

/// The new remaining estimate after a change of `delta` seconds logged (negative when work
/// is removed), or `None` to leave it alone.
fn adjusted(
    query: &AdjustQuery,
    current: Option<i64>,
    delta: i64,
) -> ApiResult<Option<Option<i64>>> {
    let current = current.unwrap_or(0);
    Ok(match query.adjust_estimate.as_deref().unwrap_or("auto") {
        "auto" => Some(Some((current - delta).max(0))),
        "new" => Some(Some(estimate(
            query.new_estimate.as_deref(),
            "newEstimate",
        )?)),
        "manual" if delta >= 0 => Some(Some(
            (current - estimate(query.reduce_by.as_deref(), "reduceBy")?).max(0),
        )),
        "manual" => Some(Some(
            current + estimate(query.increase_by.as_deref(), "increaseBy")?,
        )),
        "leave" => None,
        other => {
            return Err(ApiError::bad_request(format!(
                "Illegal value for adjustEstimate: '{other}'."
            )));
        }
    })
}

/// Saves a new remaining estimate and records the time spent change in the issue history.
async fn record(
    ctx: &mut Ctx,
    issue: &Issue,
    user: &User,
    spent_before: i64,
    new_remaining: Option<Option<i64>>,
) -> ApiResult<()> {
    let db = ctx.state.db.clone();
    let before = IssueFields::load(&db, issue).await?;
    let mut after = before.clone();
    if let Some(remaining) = new_remaining {
        after.remaining_estimate = remaining;
    }
    let custom = CustomCatalog::load(&db).await?;
    let mut changes = super::issue_fields::diff(ctx, &custom, &before, &[], &after).await?;
    let spent_after = worklogs::time_spent(&db, issue.id).await?;
    if spent_after != spent_before {
        let seconds = |v: i64| (v > 0).then(|| v.to_string());
        changes.push(ChangeItem {
            field: "timespent".into(),
            field_id: "timespent".into(),
            field_type: "jira".into(),
            from_value: seconds(spent_before),
            from_string: seconds(spent_before),
            to_value: seconds(spent_after),
            to_string: seconds(spent_after),
        });
    }
    issues::update(&db, issue.id, user.id, &after, &changes).await?;
    Ok(())
}

async fn find_worklog(state: &AppState, issue: &Issue, id: &str) -> ApiResult<Worklog> {
    let not_found = || ApiError::not_found(format!("Cannot find worklog with id: '{id}'."));
    let id: i64 = id.parse().map_err(|_| not_found())?;
    worklogs::by_id(&state.db, issue.id, id)
        .await?
        .ok_or_else(not_found)
}

/// Only the author or a site administrator may change or delete a worklog.
fn check_owner(user: &User, worklog: &Worklog) -> ApiResult<()> {
    if worklog.author_id == user.id || user.is_admin {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "You do not have the permission to edit this worklog.",
        ))
    }
}

/// The `worklog` field of an issue: the first 20 worklogs and the total.
pub async fn issue_worklog_json(ctx: &mut Ctx, issue_id: i64) -> ApiResult<Value> {
    let all = worklogs::of_issue(&ctx.state.db, issue_id).await?;
    let mut rendered = Vec::new();
    for worklog in all.iter().take(20) {
        rendered.push(ctx.worklog_json(worklog).await?);
    }
    Ok(json!({ "startAt": 0, "maxResults": 20, "total": all.len(), "worklogs": rendered }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorklogPage {
    #[serde(default)]
    start_at: i64,
    /// Jira lists up to 5000 worklogs per page by default.
    #[serde(default = "default_worklog_page")]
    max_results: i64,
}

fn default_worklog_page() -> i64 {
    5000
}

pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<WorklogPage>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let all = worklogs::of_issue(&state.db, issue.id).await?;
    let (start_at, max_results) = (page.start_at.max(0), page.max_results.clamp(0, 5000));
    let mut rendered = Vec::new();
    for worklog in all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
    {
        rendered.push(ctx.worklog_json(worklog).await?);
    }
    Ok(Json(json!({
        "startAt": start_at,
        "maxResults": max_results,
        "total": all.len(),
        "worklogs": rendered,
    })))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(query): Query<AdjustQuery>,
    Json(request): Json<WorklogRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let issue = find_issue(&state, &id_or_key).await?;
    let input = parse_request(&request, None)?;
    let new_remaining = adjusted(&query, issue.remaining_estimate, input.seconds)?;
    let spent_before = worklogs::time_spent(&state.db, issue.id).await?;
    let worklog = worklogs::create(
        &state.db,
        issue.id,
        user.id,
        input.comment.as_deref(),
        input.started,
        input.seconds,
    )
    .await?;
    let mut ctx = Ctx::new(&state, version).await?;
    record(&mut ctx, &issue, &user, spent_before, new_remaining).await?;
    Ok((StatusCode::CREATED, Json(ctx.worklog_json(&worklog).await?)))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let worklog = find_worklog(&state, &issue, &id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    Ok(Json(ctx.worklog_json(&worklog).await?))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
    Query(query): Query<AdjustQuery>,
    Json(request): Json<WorklogRequest>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let worklog = find_worklog(&state, &issue, &id).await?;
    check_owner(&user, &worklog)?;
    let input = parse_request(&request, Some(&worklog))?;
    let delta = input.seconds - worklog.time_spent_seconds;
    let new_remaining = adjusted(&query, issue.remaining_estimate, delta)?;
    let spent_before = worklogs::time_spent(&state.db, issue.id).await?;
    let updated = worklogs::update(
        &state.db,
        worklog.id,
        user.id,
        input.comment.as_deref(),
        input.started,
        input.seconds,
    )
    .await?;
    let mut ctx = Ctx::new(&state, version).await?;
    record(&mut ctx, &issue, &user, spent_before, new_remaining).await?;
    Ok(Json(ctx.worklog_json(&updated).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path((id_or_key, id)): Path<(String, String)>,
    Query(query): Query<AdjustQuery>,
) -> ApiResult<StatusCode> {
    let issue = find_issue(&state, &id_or_key).await?;
    let worklog = find_worklog(&state, &issue, &id).await?;
    check_owner(&user, &worklog)?;
    let new_remaining = adjusted(
        &query,
        issue.remaining_estimate,
        -worklog.time_spent_seconds,
    )?;
    let spent_before = worklogs::time_spent(&state.db, issue.id).await?;
    worklogs::delete(&state.db, worklog.id).await?;
    let mut ctx = Ctx::new(&state, version).await?;
    record(&mut ctx, &issue, &user, spent_before, new_remaining).await?;
    Ok(StatusCode::NO_CONTENT)
}
