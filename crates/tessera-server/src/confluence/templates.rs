//! Page templates: `/template`, `/template/page`, `/template/blueprint`, and `/template/{id}`.
//!
//! A template belongs to a space, or is global. Space administrators manage a space's templates
//! and site administrators manage global ones. Blueprints are templates Tessera ships; they can
//! be used but not changed.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::Params;
use super::access;
use super::error::{WikiError, WikiResult};
use super::render::{Expand, WikiCtx};
use super::service;
use super::spaces::find_space;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::store::templates::{self, NewTemplate, Template};
use crate::store::users::User;

fn missing(id: &str) -> WikiError {
    WikiError::not_found(format!("No template found with id {id}"))
}

/// A template the user may see: a global one, or one in a space they can read.
async fn find(state: &AppState, user: &User, id: &str) -> WikiResult<Template> {
    let number: i64 = id.parse().map_err(|_| missing(id))?;
    let template = templates::by_id(&state.db, number)
        .await?
        .ok_or_else(|| missing(id))?;
    if let Some(space) = template.space_id {
        access::readable(&state.db, user, space, || missing(id)).await?;
    }
    Ok(template)
}

/// Checks that the user may change templates in a space, or global ones.
async fn require_manage(state: &AppState, user: &User, space_id: Option<i64>) -> WikiResult<()> {
    match space_id {
        Some(space) => {
            access::require(&state.db, user, space, "administer", "space").await?;
            Ok(())
        }
        None if user.is_admin => Ok(()),
        None => Err(WikiError::new(
            StatusCode::FORBIDDEN,
            "Only site administrators can change global templates",
        )),
    }
}

async fn template_json(
    ctx: &mut WikiCtx<'_>,
    template: &Template,
    with_body: bool,
) -> WikiResult<Value> {
    let labels: Vec<Value> = template
        .label_names()
        .iter()
        .map(|name| json!({ "prefix": "global", "name": name, "id": name, "label": name }))
        .collect();
    let mut out = json!({
        "templateId": template.id.to_string(),
        "name": template.name,
        "description": template.description,
        "labels": labels,
        "templateType": "page",
        "editorVersion": "v2",
        "_links": {
            "self": ctx.api_url(&format!("/template/{}", template.id)),
        },
    });
    if let Some(blueprint) = &template.blueprint {
        out["originalTemplate"] =
            json!({ "pluginKey": "com.tessera.confluence.blueprints", "moduleKey": blueprint });
        out["referencingBlueprint"] = json!(blueprint);
    }
    if let Some(space) = template.space_id {
        out["space"] = ctx.space_by_id(space, &Expand::default()).await?;
    }
    if with_body {
        out["body"] = json!({ "storage": { "value": template.body, "representation": "storage" } });
    } else {
        out["_expandable"] = json!({ "body": "" });
    }
    Ok(out)
}

async fn list_of(
    state: &AppState,
    user: &User,
    params: &Params,
    items: Vec<Template>,
    path: &str,
) -> WikiResult<Json<Value>> {
    let (start, limit) = params.page(25, 1000);
    let more = items.len() as i64 > start + limit;
    let with_body = params.expand().has("body");
    let mut ctx = WikiCtx::new(state, user);
    let mut results = Vec::new();
    for template in items.iter().skip(start as usize).take(limit as usize) {
        results.push(template_json(&mut ctx, template, with_body).await?);
    }
    Ok(Json(ctx.collection(results, start, limit, path, more)))
}

/// `GET /template/page?spaceKey=`: a space's templates, or the global ones without a key.
pub async fn page_templates(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let space_id = match params.get("spaceKey") {
        Some(key) => Some(find_space(&state, &user, key).await?.id),
        None => None,
    };
    let items: Vec<Template> = templates::all(&state.db)
        .await?
        .into_iter()
        .filter(|t| t.blueprint.is_none() && t.space_id == space_id)
        .collect();
    list_of(&state, &user, &params, items, "/template/page").await
}

/// `GET /template/blueprint`: the blueprints Tessera ships.
pub async fn blueprints(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    if let Some(key) = params.get("spaceKey") {
        find_space(&state, &user, key).await?;
    }
    let items: Vec<Template> = templates::all(&state.db)
        .await?
        .into_iter()
        .filter(|t| t.blueprint.is_some())
        .collect();
    list_of(&state, &user, &params, items, "/template/blueprint").await
}

/// `GET /template/{id}`, with its body.
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> WikiResult<Json<Value>> {
    let template = find(&state, &user, &id).await?;
    let mut ctx = WikiCtx::new(&state, &user);
    Ok(Json(template_json(&mut ctx, &template, true).await?))
}

/// The storage body of a request: `body.storage`, or `body.atlas_doc_format`.
fn body_of(request: &Value) -> WikiResult<Option<String>> {
    let body = &request["body"];
    for representation in ["storage", "editor", "atlas_doc_format"] {
        if let Some(part) = body.get(representation) {
            return service::body_from(representation, &part["value"]).map(Some);
        }
    }
    Ok(None)
}

fn labels_of(request: &Value) -> Option<Vec<String>> {
    request["labels"].as_array().map(|labels| {
        labels
            .iter()
            .filter_map(|l| {
                l["name"]
                    .as_str()
                    .or_else(|| l.as_str())
                    .map(str::to_string)
            })
            .collect()
    })
}

fn name_of(request: &Value) -> WikiResult<String> {
    request["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .ok_or_else(|| WikiError::bad_request("A template needs a name"))
}

/// The request object. The Python client's update sends it encoded twice, as a JSON string.
fn unwrap_request(request: Value) -> Value {
    match &request {
        Value::String(text) => serde_json::from_str(text).unwrap_or(request),
        _ => request,
    }
}

fn check_type(request: &Value) -> WikiResult<()> {
    match request["templateType"].as_str() {
        None | Some("page") => Ok(()),
        Some(other) => Err(WikiError::bad_request(format!(
            "Unsupported template type: {other}. Use page."
        ))),
    }
}

/// `POST /template`: a template in `space`, or a global one.
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<Value>,
) -> WikiResult<Json<Value>> {
    let request = unwrap_request(request);
    check_type(&request)?;
    let name = name_of(&request)?;
    let space_id = match request["space"]["key"].as_str() {
        Some(key) => Some(find_space(&state, &user, key).await?.id),
        None => None,
    };
    require_manage(&state, &user, space_id).await?;
    let body = body_of(&request)?.unwrap_or_default();
    let template = templates::create(
        &state.db,
        NewTemplate {
            name: &name,
            description: request["description"].as_str().unwrap_or_default(),
            space_id,
            body: &body,
            labels: &labels_of(&request).unwrap_or_default(),
        },
    )
    .await?;
    let mut ctx = WikiCtx::new(&state, &user);
    Ok(Json(template_json(&mut ctx, &template, true).await?))
}

/// `PUT /template`: changes the template named by `templateId`.
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<Value>,
) -> WikiResult<Json<Value>> {
    let request = unwrap_request(request);
    check_type(&request)?;
    let id = request["templateId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| request["templateId"].as_i64().map(|n| n.to_string()))
        .ok_or_else(|| WikiError::bad_request("templateId is required"))?;
    let mut template = find(&state, &user, &id).await?;
    if template.blueprint.is_some() {
        return Err(WikiError::bad_request(
            "Blueprint templates cannot be changed",
        ));
    }
    require_manage(&state, &user, template.space_id).await?;
    template.name = name_of(&request)?;
    if let Some(description) = request["description"].as_str() {
        template.description = description.to_string();
    }
    if let Some(body) = body_of(&request)? {
        template.body = body;
    }
    if let Some(labels) = labels_of(&request) {
        template.labels = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());
    }
    templates::update(&state.db, &template).await?;
    let mut ctx = WikiCtx::new(&state, &user);
    Ok(Json(template_json(&mut ctx, &template, true).await?))
}

/// `DELETE /template/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> WikiResult<StatusCode> {
    let template = find(&state, &user, &id).await?;
    if template.blueprint.is_some() {
        return Err(WikiError::bad_request(
            "Blueprint templates cannot be deleted",
        ));
    }
    require_manage(&state, &user, template.space_id).await?;
    templates::delete(&state.db, template.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
