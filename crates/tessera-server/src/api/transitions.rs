//! Workflow transitions: `GET` and `POST /issue/{idOrKey}/transitions`.

use super::extract::Json;
use crate::api::access::find_issue_as;
use crate::notify::{self, IssueEvent};
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::ApiVersion;
use super::issues::{IssueRequest, find_issue, save_edit};
use super::render::Ctx;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::issues::{self, Issue, IssueFields};
use crate::store::meta::json_id;
use crate::store::projects::{self, Project};
use crate::store::users::User;
use crate::store::workflows;
use crate::time::now_ms;

/// The transitions available to an issue, in the REST API's shape.
pub async fn transitions_json(
    ctx: &Ctx,
    project: &Project,
    issue: &Issue,
) -> ApiResult<Vec<Value>> {
    let workflow =
        workflows::for_issue_type(&ctx.state.db, project.id, issue.issue_type_id).await?;
    let available = workflows::available(&ctx.state.db, workflow, issue.status_id).await?;
    Ok(available
        .iter()
        .filter_map(|t| {
            let status = ctx.catalog.status(t.to_status_id)?;
            Some(json!({
                "id": t.id.to_string(),
                "name": t.name,
                "to": ctx.status(status),
                "hasScreen": false,
                "isGlobal": true,
                "isInitial": false,
                "isAvailable": true,
                "isConditional": false,
                "isLooped": false,
            }))
        })
        .collect())
}

pub async fn list(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let project = projects::by_id(&state.db, issue.project_id)
        .await?
        .expect("issue has a project");
    let ctx = Ctx::new(&state, version).await?;
    Ok(Json(json!({
        "expand": "transitions",
        "transitions": transitions_json(&ctx, &project, &issue).await?,
    })))
}

#[derive(Deserialize)]
pub struct TransitionRequest {
    transition: Value,
    #[serde(default)]
    fields: Map<String, Value>,
    #[serde(default)]
    update: Map<String, Value>,
}

/// `POST /issue/{idOrKey}/transitions`: moves an issue to another status.
pub async fn perform(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<TransitionRequest>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "TRANSITION_ISSUES").await?;
    let transition_id = request
        .transition
        .get("id")
        .and_then(json_id)
        .ok_or_else(|| ApiError::bad_request("Missing 'transition' identifier"))?;
    let edit = IssueRequest {
        fields: request.fields,
        update: request.update,
    };
    transition(&state, version, &user, &issue, transition_id, &edit, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves an issue the user may transition to another status, applying the fields and updates
/// in `edit` too. Bulk transitions use it as well.
///
/// Like Jira's simplified workflow, moving to a done status sets the resolution (Done unless the
/// request picks one), and moving out of done clears it.
pub async fn transition(
    state: &AppState,
    version: ApiVersion,
    user: &User,
    issue: &Issue,
    transition_id: i64,
    edit: &IssueRequest,
    send_notifications: bool,
) -> ApiResult<()> {
    let project = projects::by_id(&state.db, issue.project_id)
        .await?
        .expect("issue has a project");
    let workflow = workflows::for_issue_type(&state.db, project.id, issue.issue_type_id).await?;
    let available = workflows::available(&state.db, workflow, issue.status_id).await?;
    let transition = available
        .iter()
        .find(|t| t.id == transition_id)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "Transition id '{transition_id}' is not valid for this issue."
            ))
        })?;

    let mut ctx = Ctx::new(state, version).await?;
    let entering_done = ctx.catalog.is_done(transition.to_status_id);

    // Apply the status and resolution first, then any fields sent with the transition.
    let before = IssueFields::load(&state.db, issue).await?;
    let mut after = before.clone();
    after.status_id = transition.to_status_id;
    if entering_done && after.resolution_id.is_none() {
        after.resolution_id = ctx.catalog.default_resolution();
        after.resolution_date = Some(now_ms());
    } else if !entering_done {
        after.resolution_id = None;
        after.resolution_date = None;
    }
    let custom = super::issue_fields::CustomCatalog::load(&state.db).await?;
    let changes = super::issue_fields::diff(&mut ctx, &custom, &before, &[], &after).await?;
    issues::update(&state.db, issue.id, user.id, &after, &changes).await?;

    if !edit.fields.is_empty() || !edit.update.is_empty() {
        let moved = find_issue(state, &issue.id.to_string()).await?;
        save_edit(&mut ctx, &moved, user.id, edit).await?;
    }
    if send_notifications {
        let status = ctx
            .catalog
            .status(transition.to_status_id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let current = find_issue(state, &issue.id.to_string()).await?;
        notify::issue(state, user, &current, IssueEvent::Transitioned(&status)).await;
    }
    Ok(())
}
