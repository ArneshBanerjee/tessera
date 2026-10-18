//! Board quick filters.
//!
//! Jira's public API lists them at `/rest/agile/1.0/board/{id}/quickfilter`. Changing them has
//! no public endpoint in Jira, so Tessera serves that under `/rest/tessera/1.0`.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use serde::Deserialize;
use serde_json::{Value, json};

use super::boards::find_board;
use crate::AppState;
use crate::api::Page;
use crate::api::access::require_project;
use crate::api::extract::Json;
use crate::api::search::parse_jql;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::boards::Board;
use crate::store::projects;
use crate::store::quick_filters::{self, QuickFilter};
use crate::store::users::User;

pub fn tessera_router() -> Router<AppState> {
    Router::new()
        .route("/board/{id}/quickfilter", post(create))
        .route(
            "/board/{id}/quickfilter/{filter}",
            put(update).delete(delete),
        )
}

fn filter_json(filter: &QuickFilter) -> Value {
    json!({
        "id": filter.id,
        "boardId": filter.board_id,
        "name": filter.name,
        "jql": filter.jql,
        "description": filter.description,
        "position": filter.position,
    })
}

/// A board the user may see; with `admin`, one whose project they administer.
async fn board(state: &AppState, user: &User, id: &str, admin: bool) -> ApiResult<Board> {
    let board = find_board(state, id).await?;
    if let Some(project_id) = board.project_id
        && let Some(project) = projects::by_id(&state.db, project_id).await?
    {
        let permission = if admin {
            "ADMINISTER_PROJECTS"
        } else {
            "BROWSE_PROJECTS"
        };
        require_project(state, user, &project, permission).await?;
    } else if admin && !user.is_admin {
        return Err(ApiError::forbidden(
            "Only administrators may change this board.",
        ));
    }
    Ok(board)
}

/// `GET /rest/agile/1.0/board/{id}/quickfilter`.
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let board = board(&state, &user, &id, false).await?;
    let all = quick_filters::of_board(&state.db, board.id).await?;
    let (start_at, max_results) = page.bounded(50);
    let total = all.len() as i64;
    let values: Vec<Value> = all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
        .map(filter_json)
        .collect();
    Ok(Json(json!({
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

/// `GET /rest/agile/1.0/board/{id}/quickfilter/{filter}`.
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, filter)): Path<(String, i64)>,
) -> ApiResult<Json<Value>> {
    let board = board(&state, &user, &id, false).await?;
    Ok(Json(filter_json(&find(&state, &board, filter).await?)))
}

async fn find(state: &AppState, board: &Board, id: i64) -> ApiResult<QuickFilter> {
    quick_filters::of_board(&state.db, board.id)
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| {
            ApiError::not_found(format!("Quick filter {id} does not exist on this board."))
        })
}

#[derive(Deserialize)]
pub struct FilterRequest {
    name: Option<String>,
    jql: Option<String>,
    description: Option<String>,
}

/// The JQL must parse, so a broken quick filter never reaches the board.
fn check(name: &str, jql: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::field("name", "A quick filter needs a name."));
    }
    parse_jql(jql)?;
    Ok(())
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<FilterRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let board = board(&state, &user, &id, true).await?;
    let name = request.name.unwrap_or_default();
    let jql = request.jql.unwrap_or_default();
    check(&name, &jql)?;
    let created = quick_filters::create(
        &state.db,
        board.id,
        name.trim(),
        jql.trim(),
        request.description.as_deref().unwrap_or(""),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(filter_json(&created))))
}

async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, filter)): Path<(String, i64)>,
    Json(request): Json<FilterRequest>,
) -> ApiResult<Json<Value>> {
    let board = board(&state, &user, &id, true).await?;
    let current = find(&state, &board, filter).await?;
    let name = request.name.unwrap_or(current.name);
    let jql = request.jql.unwrap_or(current.jql);
    check(&name, &jql)?;
    let description = request.description.unwrap_or(current.description);
    quick_filters::update(&state.db, filter, name.trim(), jql.trim(), &description).await?;
    Ok(Json(filter_json(&find(&state, &board, filter).await?)))
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, filter)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    let board = board(&state, &user, &id, true).await?;
    find(&state, &board, filter).await?;
    quick_filters::delete(&state.db, filter).await?;
    Ok(StatusCode::NO_CONTENT)
}
