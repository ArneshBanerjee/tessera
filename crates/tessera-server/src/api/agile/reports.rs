//! Agile reports.
//!
//! - `/rest/greenhopper/1.0/rapid/charts/sprintreport` and `.../velocity`: Jira Software's own
//!   report endpoints, which scripts and plugins use.
//! - `/rest/tessera/1.0/reports/burndown`: the burndown series for the web app.

use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::boards::find_board;
use crate::AppState;
use crate::api::access::require_project;
use crate::api::extract::Json;
use crate::api::issue_fields::CustomCatalog;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::reports::{self, Outcome, Timeline};
use crate::store::boards::Board;
use crate::store::meta::Catalog;
use crate::store::projects;
use crate::store::sprints::{self, Sprint};
use crate::store::users::{self, User};
use crate::time::format;

pub fn greenhopper_router() -> Router<AppState> {
    Router::new()
        .route("/rapid/charts/sprintreport", get(sprint_report))
        .route("/rapid/charts/velocity", get(velocity))
}

pub fn tessera_router() -> Router<AppState> {
    Router::new().route("/reports/burndown", get(burndown))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartQuery {
    rapid_view_id: Option<i64>,
    sprint_id: Option<i64>,
}

/// A board the user may see, by the `rapidViewId` parameter.
async fn board_of(state: &AppState, user: &User, id: Option<i64>) -> ApiResult<Board> {
    let id = id.ok_or_else(|| ApiError::bad_request("rapidViewId is required."))?;
    let board = find_board(state, &id.to_string()).await?;
    if let Some(project_id) = board.project_id
        && let Some(project) = projects::by_id(&state.db, project_id).await?
    {
        require_project(state, user, &project, "BROWSE_PROJECTS").await?;
    }
    Ok(board)
}

async fn sprint_of(state: &AppState, user: &User, id: Option<i64>) -> ApiResult<Sprint> {
    let id = id.ok_or_else(|| ApiError::bad_request("sprintId is required."))?;
    let sprint = sprints::by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Sprint with id {id} does not exist.")))?;
    board_of(state, user, sprint.origin_board_id).await?;
    Ok(sprint)
}

async fn estimate_field(state: &AppState) -> ApiResult<Option<i64>> {
    let custom = CustomCatalog::load(&state.db).await?;
    Ok(custom
        .fields
        .iter()
        .find(|f| f.name == "Story point estimate")
        .map(|f| f.id))
}

fn stat(value: Option<f64>) -> Value {
    match value {
        Some(v) => {
            json!({ "value": v, "text": if v.fract() == 0.0 { format!("{v:.0}") } else { v.to_string() } })
        }
        None => json!({ "text": "null" }),
    }
}

fn sum(items: &[Outcome], pick: impl Fn(&Outcome) -> Option<f64>) -> Value {
    stat(Some(items.iter().filter_map(pick).sum()))
}

/// An issue in Jira Software's report shape.
async fn report_issue(
    state: &AppState,
    catalog: &Catalog,
    outcome: &Outcome<'_>,
    end: i64,
    field: Option<i64>,
) -> ApiResult<Value> {
    let issue = &outcome.timeline.issue;
    let status_id = outcome.timeline.status(end);
    let status = catalog.status(status_id);
    let assignee = match issue.assignee_id {
        Some(id) => users::by_id(&state.db, id).await?,
        None => None,
    };
    let estimate = |v: Option<f64>| json!({ "statFieldId": field.map(|f| format!("customfield_{f}")), "statFieldValue": stat(v) });
    Ok(json!({
        "id": issue.id,
        "key": issue.key,
        "summary": issue.summary,
        "typeId": issue.issue_type_id.to_string(),
        "typeName": catalog.issue_type(issue.issue_type_id).map(|t| t.name.clone()),
        "statusId": status_id.to_string(),
        "statusName": status.map(|s| s.name.clone()),
        "done": catalog.is_done(status_id),
        "assignee": assignee.as_ref().map(|u| u.account_id.clone()),
        "assigneeName": assignee.as_ref().map(|u| u.display_name.clone()),
        "projectId": issue.project_id,
        "hidden": false,
        "estimateStatisticRequired": false,
        "estimateStatistic": estimate(outcome.initial_estimate),
        "currentEstimateStatistic": estimate(outcome.final_estimate),
    }))
}

fn report_sprint(sprint: &Sprint) -> Value {
    json!({
        "id": sprint.id,
        "sequence": sprint.sequence,
        "name": sprint.name,
        "state": sprint.state.to_uppercase(),
        "goal": sprint.goal,
        "startDate": sprint.start_date.map(format),
        "endDate": sprint.end_date.map(format),
        "completeDate": sprint.complete_date.map(format),
        "activatedDate": sprint.activated_at.map(format),
    })
}

/// `GET /rest/greenhopper/1.0/rapid/charts/sprintreport?rapidViewId=&sprintId=`.
pub async fn sprint_report(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ChartQuery>,
) -> ApiResult<Json<Value>> {
    let sprint = sprint_of(&state, &user, query.sprint_id).await?;
    let catalog = Catalog::load(&state.db).await?;
    let field = estimate_field(&state).await?;
    let timelines = reports::timelines(&state.db, &sprint, field).await?;
    let report = reports::sprint_report(&sprint, &catalog, &timelines);
    let end = reports::sprint_end(&sprint);
    let mut lists = Map::new();
    for (name, items) in [
        ("completedIssues", &report.completed),
        ("issuesNotCompletedInCurrentSprint", &report.not_completed),
        ("puntedIssues", &report.punted),
    ] {
        let mut rendered = Vec::new();
        for outcome in items {
            rendered.push(report_issue(&state, &catalog, outcome, end, field).await?);
        }
        lists.insert(name.into(), json!(rendered));
    }
    let added: Map<String, Value> = report
        .completed
        .iter()
        .chain(&report.not_completed)
        .chain(&report.punted)
        .filter(|o| o.added_during_sprint)
        .map(|o| (o.timeline.issue.key.clone(), json!(true)))
        .collect();
    let all: Vec<&Outcome> = report
        .completed
        .iter()
        .chain(&report.not_completed)
        .collect();
    let mut contents = Value::Object(lists);
    contents["issuesCompletedInAnotherSprint"] = json!([]);
    contents["completedIssuesInitialEstimateSum"] = sum(&report.completed, |o| o.initial_estimate);
    contents["completedIssuesEstimateSum"] = sum(&report.completed, |o| o.final_estimate);
    contents["issuesNotCompletedInitialEstimateSum"] =
        sum(&report.not_completed, |o| o.initial_estimate);
    contents["issuesNotCompletedEstimateSum"] = sum(&report.not_completed, |o| o.final_estimate);
    contents["allIssuesEstimateSum"] =
        stat(Some(all.iter().filter_map(|o| o.final_estimate).sum()));
    contents["puntedIssuesInitialEstimateSum"] = sum(&report.punted, |o| o.initial_estimate);
    contents["puntedIssuesEstimateSum"] = sum(&report.punted, |o| o.final_estimate);
    contents["issueKeysAddedDuringSprint"] = Value::Object(added);
    Ok(Json(
        json!({ "contents": contents, "sprint": report_sprint(&sprint) }),
    ))
}

/// `GET /rest/greenhopper/1.0/rapid/charts/velocity?rapidViewId=`: committed and completed
/// story points of the board's last seven completed sprints.
pub async fn velocity(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ChartQuery>,
) -> ApiResult<Json<Value>> {
    let board = board_of(&state, &user, query.rapid_view_id).await?;
    let catalog = Catalog::load(&state.db).await?;
    let field = estimate_field(&state).await?;
    let closed: Vec<Sprint> = sprints::of_board(&state.db, board.id)
        .await?
        .into_iter()
        .filter(|s| s.state == "closed")
        .collect();
    let recent: Vec<&Sprint> = closed.iter().rev().take(7).collect();
    let mut entries = Map::new();
    let mut listed = Vec::new();
    for sprint in recent.iter().rev() {
        let timelines: Vec<Timeline> = reports::timelines(&state.db, sprint, field).await?;
        let (committed, completed) = reports::velocity(sprint, &catalog, &timelines);
        entries.insert(
            sprint.id.to_string(),
            json!({ "estimated": stat(Some(committed)), "completed": stat(Some(completed)) }),
        );
        listed.push(json!({
            "id": sprint.id,
            "sequence": sprint.sequence,
            "name": sprint.name,
            "state": "CLOSED",
            "goal": sprint.goal,
            "linkedPagesCount": 0,
        }));
    }
    Ok(Json(
        json!({ "sprints": listed, "velocityStatEntries": entries }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BurndownQuery {
    sprint_id: Option<i64>,
}

/// `GET /rest/tessera/1.0/reports/burndown?sprintId=`: the story points left over the sprint,
/// and the straight guideline from the start to zero at the planned end.
pub async fn burndown(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<BurndownQuery>,
) -> ApiResult<Json<Value>> {
    let sprint = sprint_of(&state, &user, query.sprint_id).await?;
    if sprint.activated_at.is_none() && sprint.start_date.is_none() {
        return Err(ApiError::bad_request("The sprint has not started."));
    }
    let catalog = Catalog::load(&state.db).await?;
    let field = estimate_field(&state).await?;
    let timelines = reports::timelines(&state.db, &sprint, field).await?;
    let points = reports::burndown(&sprint, &catalog, &timelines);
    let start = points.first().copied().unwrap_or_default();
    Ok(Json(json!({
        "sprint": report_sprint(&sprint),
        "points": points.iter().map(|(t, v)| json!({ "time": format(*t), "remaining": v })).collect::<Vec<_>>(),
        "guideline": [
            { "time": format(start.0), "remaining": start.1 },
            { "time": sprint.end_date.map(format), "remaining": 0 },
        ],
    })))
}
