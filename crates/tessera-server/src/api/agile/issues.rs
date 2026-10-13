//! Agile issue operations: the Agile view of an issue, ranking, estimation, and the backlog.

use crate::api::extract::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::boards::find_board;
use super::{MoveRequest, agile_issue_json, rank_relative, requested_issues, set_sprint};
use crate::AppState;
use crate::api::ApiVersion;
use crate::api::issue_fields::CustomCatalog;
use crate::api::issues::{IssueRequest, find_issue, save_edit};
use crate::api::render::Ctx;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::fields::CustomField;
use crate::store::issues;

/// An issue with its Agile fields.
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let mut ctx = Ctx::new(&state, ApiVersion::V2).await?.viewed_by(user.id);
    let custom = CustomCatalog::load(&state.db).await?;
    Ok(Json(agile_issue_json(&mut ctx, &custom, &issue).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankRequest {
    #[serde(default)]
    issues: Vec<String>,
    rank_before_issue: Option<String>,
    rank_after_issue: Option<String>,
}

/// Ranks issues before or after another issue. Returns 204 when every issue moved.
pub async fn rank(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Json(request): Json<RankRequest>,
) -> ApiResult<StatusCode> {
    if request.rank_before_issue.is_none() && request.rank_after_issue.is_none() {
        return Err(ApiError::bad_request(
            "Specify rankBeforeIssue or rankAfterIssue.",
        ));
    }
    let issues = requested_issues(&state, &request.issues).await?;
    rank_relative(
        &state,
        &issues,
        request.rank_before_issue.as_deref(),
        request.rank_after_issue.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimationQuery {
    board_id: Option<i64>,
}

/// The estimation field of a board: Story point estimate.
async fn estimation_field(state: &AppState, board_id: Option<i64>) -> ApiResult<CustomField> {
    if let Some(board_id) = board_id {
        find_board(state, &board_id.to_string()).await?;
    }
    CustomCatalog::load(&state.db)
        .await?
        .fields
        .into_iter()
        .find(|f| f.name == "Story point estimate")
        .ok_or_else(|| ApiError::bad_request("The board has no estimation field."))
}

async fn estimation_json(state: &AppState, issue_id: i64, field: &CustomField) -> ApiResult<Value> {
    let stored = issues::custom_values(&state.db, issue_id).await?;
    let value = stored
        .iter()
        .find(|(id, _)| *id == field.id)
        .and_then(|(_, v)| serde_json::from_str::<f64>(v).ok());
    Ok(json!({
        "fieldId": crate::api::fields::custom_field_key(field.id),
        "value": value.map(|v| v.to_string()),
    }))
}

pub async fn get_estimation(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(query): Query<EstimationQuery>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let field = estimation_field(&state, query.board_id).await?;
    Ok(Json(estimation_json(&state, issue.id, &field).await?))
}

/// Sets the estimate, `{"value": "8"}`. An empty value clears it.
pub async fn set_estimation(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(query): Query<EstimationQuery>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue(&state, &id_or_key).await?;
    let field = estimation_field(&state, query.board_id).await?;
    let number = match body.get("value") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(text)) if text.trim().is_empty() => Value::Null,
        Some(Value::String(text)) => json!(
            text.trim()
                .parse::<f64>()
                .map_err(|_| ApiError::field("value", "The estimate must be a number."))?
        ),
        Some(Value::Number(n)) => Value::Number(n.clone()),
        Some(_) => return Err(ApiError::field("value", "The estimate must be a number.")),
    };
    let fields = Map::from_iter([(crate::api::fields::custom_field_key(field.id), number)]);
    let mut ctx = Ctx::new(&state, ApiVersion::V2).await?;
    save_edit(
        &mut ctx,
        &issue,
        user.id,
        &IssueRequest {
            fields,
            update: Map::new(),
        },
    )
    .await?;
    Ok(Json(estimation_json(&state, issue.id, &field).await?))
}

/// Moves issues to the backlog: out of their future and active sprints.
pub async fn to_backlog(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    for issue in requested_issues(&state, &request.issues).await? {
        set_sprint(&state, &user, &issue, None).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Moves issues to a board's backlog, optionally ranking them.
pub async fn to_board_backlog(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(board_id): Path<String>,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    find_board(&state, &board_id).await?;
    let issues = requested_issues(&state, &request.issues).await?;
    for issue in &issues {
        set_sprint(&state, &user, issue, None).await?;
    }
    rank_relative(
        &state,
        &issues,
        request.rank_before_issue.as_deref(),
        request.rank_after_issue.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
