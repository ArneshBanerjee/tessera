//! Sharing for saved filters and dashboards: who besides the owner may see one.
//!
//! - `global`: everyone, signed in or not.
//! - `loggedin` (also sent as `authenticated`): every signed-in user.
//! - `project`: people who can browse the project.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::access::Access;
use super::render::Ctx;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::store::filters::Share;
use crate::store::projects;
use crate::store::users::User;

/// Parses `sharePermissions` from a request.
pub async fn parse(state: &AppState, value: Option<&Value>) -> ApiResult<Vec<Share>> {
    let mut shares = Vec::new();
    for item in value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let share_type = match item.get("type").and_then(Value::as_str) {
            Some("global") => "global",
            Some("loggedin" | "authenticated") => "loggedin",
            Some("project") => "project",
            other => {
                return Err(ApiError::field(
                    "sharePermissions",
                    format!("The share type {other:?} is not supported."),
                ));
            }
        };
        let project_id = if share_type == "project" {
            let reference = item
                .get("project")
                .and_then(|p| p.get("id").or_else(|| p.get("key")))
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .ok_or_else(|| {
                    ApiError::field("sharePermissions", "A project share needs a project.")
                })?;
            let project = projects::by_id_or_key(&state.db, &reference)
                .await?
                .ok_or_else(|| {
                    ApiError::field(
                        "sharePermissions",
                        format!("The project '{reference}' does not exist."),
                    )
                })?;
            Some(project.id)
        } else {
            None
        };
        shares.push(Share {
            share_type: share_type.into(),
            project_id,
        });
    }
    Ok(shares)
}

/// One share, as Jira shows it. Shares have no ids of their own, so they are numbered.
pub async fn share_json(ctx: &Ctx, share: &Share, index: usize) -> ApiResult<Value> {
    let mut value = json!({ "id": index + 1, "type": share.share_type });
    if let Some(project_id) = share.project_id
        && let Some(project) = projects::by_id(&ctx.state.db, project_id).await?
    {
        value["project"] = ctx.project_ref(&project);
    }
    Ok(value)
}

pub async fn shares_json(ctx: &Ctx, shares: &[Share]) -> ApiResult<Vec<Value>> {
    let mut out = Vec::with_capacity(shares.len());
    for (index, share) in shares.iter().enumerate() {
        out.push(share_json(ctx, share, index).await?);
    }
    Ok(out)
}

/// Whether any of the shares reaches the user (`None` for anonymous visitors).
pub async fn reaches(db: &SqlitePool, user: Option<&User>, shares: &[Share]) -> ApiResult<bool> {
    for share in shares {
        let reached = match (share.share_type.as_str(), user) {
            ("global", _) => true,
            (_, None) => false,
            ("project", Some(user)) => match share.project_id {
                Some(id) => match projects::by_id(db, id).await? {
                    Some(project) => Access::load(db, user, &project)
                        .await?
                        .can("BROWSE_PROJECTS"),
                    None => false,
                },
                None => false,
            },
            _ => true,
        };
        if reached {
            return Ok(true);
        }
    }
    Ok(false)
}
