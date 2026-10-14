//! Boards: listing, creation, configuration, and the issue views of a board.

use crate::api::access::find_issue_as;
use crate::api::extract::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tessera_jql::ast::{Clause, Operand, Operator};

use super::{
    IssueQuery, PageQuery, agile_url, clause, epic_json, extra_clause, function, is_epic,
    issue_page, page, properties, ranked, sprint_json, value,
};
use crate::AppState;
use crate::api::ApiVersion;
use crate::api::access::browsable_projects;
use crate::api::render::Ctx;
use crate::api::search::parse_jql;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::boards::{self, Board};
use crate::store::filters;
use crate::store::meta::Catalog;
use crate::store::projects::{self, Project};
use crate::store::properties::Entity;
use crate::store::users::User;
use crate::store::{issues, sprints, versions, workflows};

async fn board_json(ctx: &Ctx, board: &Board) -> ApiResult<Value> {
    let mut value = json!({
        "id": board.id,
        "self": agile_url(ctx, &format!("/board/{}", board.id)),
        "name": board.name,
        "type": board.board_type,
    });
    if let Some(project) = board_project(&ctx.state.db, board).await? {
        value["location"] = json!({
            "projectId": project.id,
            "displayName": format!("{} ({})", project.name, project.key),
            "projectName": project.name,
            "projectKey": project.key,
            "projectTypeKey": project.project_type_key,
            "avatarURI": format!("{}/avatar/project/{}?size=48", ctx.state.base_url, project.key),
            "name": format!("{} ({})", project.name, project.key),
        });
    }
    Ok(value)
}

async fn board_project(db: &SqlitePool, board: &Board) -> ApiResult<Option<Project>> {
    Ok(match board.project_id {
        Some(id) => projects::by_id(db, id).await?,
        None => None,
    })
}

fn board_not_found() -> ApiError {
    ApiError::not_found(
        "The requested board cannot be viewed because it either does not exist or you do not have permission to view it.",
    )
}

pub(super) async fn find_board(state: &AppState, id: &str) -> ApiResult<Board> {
    let id: i64 = id.parse().map_err(|_| board_not_found())?;
    boards::by_id(&state.db, id)
        .await?
        .ok_or_else(board_not_found)
}

/// The board's own query: its filter's JQL.
async fn board_clause(state: &AppState, board: &Board) -> ApiResult<Option<Clause>> {
    let filter = filters::by_id(&state.db, board.filter_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("The board's filter no longer exists."))?;
    Ok(parse_jql(&filter.jql)?.filter)
}

/// Creates a board on a filter, with columns from the workflow of its project (or all statuses).
pub async fn create_board(
    db: &SqlitePool,
    name: &str,
    board_type: &str,
    filter_id: i64,
    project: Option<&Project>,
) -> ApiResult<Board> {
    let catalog = Catalog::load(db).await?;
    let status_ids = match project {
        Some(project) => workflows::project_statuses(db, project.id).await?,
        None => catalog.statuses.iter().map(|s| s.id).collect(),
    };
    let statuses: Vec<(i64, String)> = status_ids
        .iter()
        .filter_map(|id| {
            let status = catalog.status(*id)?;
            let category = catalog.status_category(status.category_id)?;
            Some((status.id, category.key.clone()))
        })
        .collect();
    Ok(boards::create(
        db,
        name,
        board_type,
        filter_id,
        project.map(|p| p.id),
        &statuses,
    )
    .await?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(default)]
    start_at: i64,
    #[serde(default = "super::default_page")]
    max_results: i64,
    #[serde(rename = "type")]
    board_type: Option<String>,
    name: Option<String>,
    project_key_or_id: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let browsable = browsable_projects(&state.db, &user).await?;
    let project_id = match &query.project_key_or_id {
        Some(reference) => Some(
            projects::by_id_or_key(&state.db, reference)
                .await?
                .map(|p| p.id)
                .unwrap_or(-1),
        ),
        None => None,
    };
    let types: Vec<String> = query
        .board_type
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let mut values = Vec::new();
    for board in boards::all(&state.db).await? {
        let type_ok = types.is_empty() || types.contains(&board.board_type);
        let name_ok = query
            .name
            .as_ref()
            .is_none_or(|n| board.name.to_lowercase().contains(&n.to_lowercase()));
        let project_ok = project_id.is_none_or(|p| board.project_id == Some(p));
        // Boards of projects the user cannot browse are hidden.
        let visible = match (&browsable, board.project_id) {
            (Some(ids), Some(project)) => ids.contains(&project),
            _ => true,
        };
        if type_ok && name_ok && project_ok && visible {
            values.push(board_json(&ctx, &board).await?);
        }
    }
    Ok(Json(page(values, query.start_at, query.max_results)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    name: Option<String>,
    #[serde(rename = "type")]
    board_type: Option<String>,
    /// Clients send the filter id as a number or a string.
    filter_id: Option<Value>,
    location: Option<Value>,
}

/// Creates a board. The filter must be visible to the caller.
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<CreateRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::field("name", "The board name is required."))?;
    let board_type = request.board_type.as_deref().unwrap_or("");
    if !matches!(board_type, "scrum" | "kanban") {
        return Err(ApiError::field(
            "type",
            "The board type must be scrum or kanban.",
        ));
    }
    let filter_id = request
        .filter_id
        .as_ref()
        .and_then(crate::store::meta::json_id)
        .ok_or_else(|| ApiError::field("filterId", "The filter id is required."))?;
    let visible = crate::api::filters::visible(&state.db, Some(&user)).await?;
    if !visible.iter().any(|f| f.id == filter_id) {
        return Err(ApiError::field(
            "filterId",
            "The filter does not exist or you cannot see it.",
        ));
    }
    // A board lives in a project, or (location type "user") under its creator.
    let project_reference = request
        .location
        .as_ref()
        .filter(|location| location.get("type").and_then(Value::as_str) != Some("user"))
        .map(|location| {
            location
                .get("projectKeyOrId")
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .ok_or_else(|| ApiError::field("location", "The location needs projectKeyOrId."))
        })
        .transpose()?;
    let project = match project_reference {
        Some(reference) => Some(
            projects::by_id_or_key(&state.db, &reference)
                .await?
                .ok_or_else(|| {
                    ApiError::field(
                        "location",
                        format!("The project '{reference}' does not exist."),
                    )
                })?,
        ),
        None => None,
    };
    let board = create_board(&state.db, name, board_type, filter_id, project.as_ref()).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok((StatusCode::CREATED, Json(board_json(&ctx, &board).await?)))
}

pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    if let Some(ids) = browsable_projects(&state.db, &user).await?
        && board.project_id.is_some_and(|p| !ids.contains(&p))
    {
        return Err(board_not_found());
    }
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    Ok(Json(board_json(&ctx, &board).await?))
}

/// Deletes a board. Its filter and issues stay. Site administrators and the filter owner only.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let board = find_board(&state, &id).await?;
    let owner = filters::by_id(&state.db, board.filter_id)
        .await?
        .is_some_and(|f| f.owner_id == user.id);
    if !owner && !user.is_admin {
        return Err(ApiError::forbidden(
            "You do not have permission to delete this board.",
        ));
    }
    boards::delete(&state.db, board.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn by_filter(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(filter_id): Path<i64>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let mut values = Vec::new();
    for board in boards::all(&state.db)
        .await?
        .iter()
        .filter(|b| b.filter_id == filter_id)
    {
        values.push(json!({
            "id": board.id,
            "self": agile_url(&ctx, &format!("/board/{}", board.id)),
            "name": board.name,
        }));
    }
    Ok(Json(page(values, query.start_at, query.max_results)))
}

pub async fn configuration(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let columns: Vec<Value> = boards::columns(&state.db, board.id)
        .await?
        .iter()
        .map(|column| {
            let mut value = json!({
                "name": column.name,
                "statuses": column.status_ids.iter().map(|id| json!({
                    "id": id.to_string(),
                    "self": ctx.api_url(&format!("/status/{id}")),
                })).collect::<Vec<_>>(),
            });
            if let Some(min) = column.min {
                value["min"] = json!(min);
            }
            if let Some(max) = column.max {
                value["max"] = json!(max);
            }
            value
        })
        .collect();
    let custom = crate::store::fields::CustomCatalog::load(&state.db).await?;
    let estimation = custom
        .fields
        .iter()
        .find(|f| f.name == "Story point estimate");
    let rank = custom.fields.iter().find(|f| f.field_type == "rank");
    let mut value = board_json(&ctx, &board).await?;
    value["filter"] = json!({
        "id": board.filter_id.to_string(),
        "self": ctx.api_url(&format!("/filter/{}", board.filter_id)),
    });
    value["columnConfig"] = json!({ "columns": columns, "constraintType": "none" });
    if let Some(field) = estimation {
        value["estimation"] = json!({
            "type": "field",
            "field": { "fieldId": crate::api::fields::custom_field_key(field.id), "displayName": field.name },
        });
    }
    if let Some(field) = rank {
        value["ranking"] = json!({ "rankCustomFieldId": field.id });
    }
    if let Some(location) = value.get("location").cloned() {
        value["location"] = json!({
            "type": "project",
            "key": location["projectKey"],
            "id": location["projectId"].to_string(),
            "self": ctx.api_url(&format!("/project/{}", location["projectId"])),
            "name": location["projectName"],
        });
    }
    Ok(Json(value))
}

/// All issues on the board, by rank.
pub async fn issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let query = ranked(vec![
        board_clause(&state, &board).await?,
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

/// The statuses of the board's last column. Issues in them count as complete.
async fn done_statuses(db: &SqlitePool, board: &Board) -> ApiResult<Vec<i64>> {
    Ok(boards::columns(db, board.id)
        .await?
        .last()
        .map(|c| c.status_ids.clone())
        .unwrap_or_default())
}

/// The backlog: incomplete issues of the board that are in no open sprint.
pub async fn backlog(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    // In no active (openSprints) or future sprint.
    let not_in_open_sprint = Clause::Not(Box::new(Clause::Or(vec![
        clause("sprint", Operator::In, function("openSprints", &[])),
        clause("sprint", Operator::In, function("futureSprints", &[])),
    ])));

    let done = done_statuses(&state.db, &board).await?;
    let incomplete = (!done.is_empty()).then(|| {
        clause(
            "status",
            Operator::NotIn,
            Operand::List(done.iter().map(|id| value(id.to_string())).collect()),
        )
    });
    let query = ranked(vec![
        board_clause(&state, &board).await?,
        Some(not_in_open_sprint),
        incomplete,
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

fn require_scrum(board: &Board) -> ApiResult<()> {
    if board.board_type == "scrum" {
        Ok(())
    } else {
        Err(ApiError::bad_request("The board does not support sprints"))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintListQuery {
    #[serde(default)]
    start_at: i64,
    #[serde(default = "super::default_page")]
    max_results: i64,
    state: Option<String>,
}

pub async fn sprints(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<SprintListQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    require_scrum(&board)?;
    let states: Vec<String> = query
        .state
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let values: Vec<Value> = sprints::of_board(&state.db, board.id)
        .await?
        .iter()
        .filter(|s| states.is_empty() || states.contains(&s.state))
        .map(|s| sprint_json(&ctx, s))
        .collect();
    Ok(Json(page(values, query.start_at, query.max_results)))
}

pub async fn sprint_issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, sprint_id)): Path<(String, i64)>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    sprints::by_id(&state.db, sprint_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Sprint with id {sprint_id} does not exist."))?;
    let query = ranked(vec![
        board_clause(&state, &board).await?,
        Some(clause(
            "sprint",
            Operator::Equals,
            value(sprint_id.to_string()),
        )),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpicListQuery {
    #[serde(default)]
    start_at: i64,
    #[serde(default = "super::default_page")]
    max_results: i64,
    done: Option<bool>,
}

/// The epics in the board's project.
pub async fn epics(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<EpicListQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let project_ids: Vec<i64> = match board.project_id {
        Some(id) => vec![id],
        None => projects::list(&state.db)
            .await?
            .iter()
            .map(|p| p.id)
            .collect(),
    };
    let mut values = Vec::new();
    for project_id in project_ids {
        let candidates: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM issues WHERE project_id = ? ORDER BY lexo_rank")
                .bind(project_id)
                .fetch_all(&state.db)
                .await?;
        for issue in issues::by_ids(&state.db, &candidates).await? {
            if !is_epic(&ctx.catalog, &issue) {
                continue;
            }
            let epic = epic_json(&ctx, &issue).await?;
            if query.done.is_none_or(|done| epic["done"] == json!(done)) {
                values.push(epic);
            }
        }
    }
    Ok(Json(page(values, query.start_at, query.max_results)))
}

pub async fn epic_issues(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, epic_id)): Path<(String, String)>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let epic = find_issue_as(&state, &user, &epic_id, "BROWSE_PROJECTS").await?;
    let query = ranked(vec![
        board_clause(&state, &board).await?,
        Some(clause("parent", Operator::Equals, value(epic.key))),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

/// The clause for issues that are not in an epic. In Jira's hierarchy only an epic can be the
/// parent of a standard issue, so these are standard, non-epic issues without a parent.
pub(super) fn without_epic() -> Clause {
    Clause::And(vec![
        clause("parent", Operator::Is, Operand::Empty),
        clause(
            "issuetype",
            Operator::In,
            function("standardIssueTypes", &[]),
        ),
        clause("issuetype", Operator::NotEquals, value("Epic")),
    ])
}

pub async fn issues_without_epic(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Query(params): Query<IssueQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let query = ranked(vec![
        board_clause(&state, &board).await?,
        Some(without_epic()),
        extra_clause(params.jql.as_deref())?,
    ]);
    Ok(Json(issue_page(&state, &user, &query, &params).await?))
}

pub async fn projects(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let values = match board_project(&state.db, &board).await? {
        Some(project) => vec![ctx.project_ref(&project)],
        None => Vec::new(),
    };
    Ok(Json(page(values, query.start_at, query.max_results)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionListQuery {
    #[serde(default)]
    start_at: i64,
    #[serde(default = "super::default_page")]
    max_results: i64,
    released: Option<bool>,
}

pub async fn versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<VersionListQuery>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    let values = match board.project_id {
        Some(project_id) => versions::for_project(&state.db, project_id)
            .await?
            .iter()
            .filter(|v| query.released.is_none_or(|r| v.released == r))
            .map(|v| ctx.version_json(v))
            .collect(),
        None => Vec::new(),
    };
    Ok(Json(page(values, query.start_at, query.max_results)))
}

/// Quick filters are not configurable yet, so every board has none.
pub async fn quick_filters(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    find_board(&state, &id).await?;
    Ok(Json(page(Vec::new(), query.start_at, query.max_results)))
}

/// Which board features are on: sprints and the backlog for scrum boards; estimation always.
pub async fn features(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let scrum = board.board_type == "scrum";
    let feature = |name: &str, enabled: bool| {
        json!({
            "boardFeature": name,
            "boardId": board.id,
            "state": if enabled { "ENABLED" } else { "DISABLED" },
            "localisedName": name,
            "featureType": "BASIC",
        })
    };
    Ok(Json(json!({
        "features": [
            feature("SIMPLE_ROADMAP", false),
            feature("BACKLOG", scrum),
            feature("SPRINTS", scrum),
            feature("ESTIMATION", true),
        ]
    })))
}

pub async fn property_keys(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    let ctx = Ctx::new(&state, ApiVersion::V2).await?;
    properties::keys(
        &state,
        Entity::Board,
        board.id,
        &agile_url(&ctx, &format!("/board/{}", board.id)),
    )
    .await
}

pub async fn get_property(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let board = find_board(&state, &id).await?;
    properties::get(&state, Entity::Board, board.id, &key).await
}

pub async fn set_property(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, key)): Path<(String, String)>,
    Json(value): Json<Value>,
) -> ApiResult<StatusCode> {
    let board = find_board(&state, &id).await?;
    check_board_admin(&state, &user, &board).await?;
    properties::set(&state, Entity::Board, board.id, &key, &value).await
}

pub async fn delete_property(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let board = find_board(&state, &id).await?;
    check_board_admin(&state, &user, &board).await?;
    properties::delete(&state, Entity::Board, board.id, &key).await
}

/// Board administrators: the owner of its filter and site administrators.
async fn check_board_admin(state: &AppState, user: &User, board: &Board) -> ApiResult<()> {
    let owner = filters::by_id(&state.db, board.filter_id)
        .await?
        .is_some_and(|f| f.owner_id == user.id);
    if owner || user.is_admin {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "You do not have permission to change this board.",
        ))
    }
}
