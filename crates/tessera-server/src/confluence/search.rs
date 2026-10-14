//! CQL search: `/content/search` returns content; `/search` returns search results, which
//! can also be spaces.

use axum::extract::State;
use serde_json::{Value, json};
use sqlx::Arguments;
use sqlx::sqlite::SqliteArguments;

use super::Params;
use super::access;
use super::cql::{self, Compiled, CqlContext, Target};
use super::error::{WikiError, WikiResult};
use super::render::WikiCtx;
use crate::AppState;
use crate::api::extract::Json;
use crate::auth::CurrentUser;
use crate::jql::Bind;
use crate::store::content::{self, Content};
use crate::store::spaces;
use crate::store::users::{self, User};
use crate::time::{format_iso, now_ms};

fn arguments(compiled: &Compiled) -> SqliteArguments {
    let mut args = SqliteArguments::default();
    for bind in &compiled.binds {
        let added = match bind {
            Bind::Int(value) => args.add(*value),
            Bind::Real(value) => args.add(*value),
            Bind::Text(value) => args.add(value.clone()),
        };
        added.expect("SQLite accepts these types");
    }
    args
}

async fn compile(
    state: &AppState,
    user: &User,
    cql_text: &str,
    target: Target,
) -> WikiResult<Compiled> {
    let all_users = users::all(&state.db).await?;
    let ctx = CqlContext {
        users: &all_users,
        current_user: Some(user.id),
        now: now_ms(),
    };
    let mut compiled =
        cql::compile(cql_text, &ctx, target).map_err(|e| WikiError::bad_request(e.0))?;
    // Content and spaces the user cannot read are never found.
    if let Some(ids) = access::readable_spaces(&state.db, user).await? {
        let column = match target {
            Target::Content => "c.space_id",
            Target::Spaces => "s.id",
        };
        compiled.filter = if ids.is_empty() {
            format!("({}) AND 0 = 1", compiled.filter)
        } else {
            let marks = vec!["?"; ids.len()].join(", ");
            format!("({}) AND {column} IN ({marks})", compiled.filter)
        };
        compiled.binds.extend(ids.into_iter().map(Bind::Int));
    }
    Ok(compiled)
}

/// Ids of current content matching the query, in order.
async fn content_ids(state: &AppState, compiled: &Compiled) -> WikiResult<Vec<i64>> {
    // Safe: the CQL compiler emits only its own SQL text. Every user-supplied value is bound.
    Ok(sqlx::query_scalar_with(
        sqlx::AssertSqlSafe(format!(
            "SELECT c.id FROM content c JOIN spaces s ON s.id = c.space_id
             WHERE c.status = 'current' AND {} ORDER BY {}",
            compiled.filter, compiled.order_by
        )),
        arguments(compiled),
    )
    .fetch_all(&state.db)
    .await?)
}

async fn space_ids(state: &AppState, compiled: &Compiled) -> WikiResult<Vec<i64>> {
    // Safe: as above.
    Ok(sqlx::query_scalar_with(
        sqlx::AssertSqlSafe(format!(
            "SELECT s.id FROM spaces s WHERE {} ORDER BY {}",
            compiled.filter, compiled.order_by
        )),
        arguments(compiled),
    )
    .fetch_all(&state.db)
    .await?)
}

async fn load(state: &AppState, ids: &[i64]) -> WikiResult<Vec<Content>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(found) = content::by_id(&state.db, *id).await? {
            out.push(found);
        }
    }
    Ok(out)
}

fn required_cql(params: &Params) -> WikiResult<String> {
    params
        .get("cql")
        .map(str::to_string)
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| WikiError::bad_request("cql query parameter is required"))
}

/// `GET /content/search?cql=...`
pub async fn content_search(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let cql_text = required_cql(&params)?;
    let (start, limit) = params.page(25, 200);
    let compiled = compile(&state, &user, &cql_text, Target::Content).await?;
    let ids = content_ids(&state, &compiled).await?;
    let more = ids.len() as i64 > start + limit;
    let page: Vec<i64> = ids
        .into_iter()
        .skip(start as usize)
        .take(limit as usize)
        .collect();
    let expand = params.expand();
    let mut ctx = WikiCtx::new(&state);
    let mut results = Vec::with_capacity(page.len());
    for found in load(&state, &page).await? {
        results.push(ctx.content(&found, &expand).await?);
    }
    let path = format!(
        "/content/search?cql={}",
        form_urlencoded::byte_serialize(cql_text.as_bytes()).collect::<String>()
    );
    let mut out = ctx.collection(results, start, limit, &path, more);
    out["cqlQuery"] = json!(cql_text);
    Ok(Json(out))
}

/// A short plain-text excerpt of a body.
fn excerpt(body: &str) -> String {
    let text = content::plain_text(body);
    let mut out: String = text.chars().take(200).collect();
    if text.chars().count() > 200 {
        out.push('…');
    }
    out
}

/// `GET /search?cql=...`: content and spaces, each wrapped in a search result.
pub async fn search(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    params: Params,
) -> WikiResult<Json<Value>> {
    let cql_text = required_cql(&params)?;
    let (start, limit) = params.page(25, 200);
    let compiled = compile(&state, &user, &cql_text, Target::Content).await?;
    // Spaces come first, then content.
    let mut hits: Vec<(bool, i64)> = Vec::new();
    if compiled.spaces {
        let spaces_query = compile(&state, &user, &cql_text, Target::Spaces).await?;
        hits.extend(
            space_ids(&state, &spaces_query)
                .await?
                .into_iter()
                .map(|id| (true, id)),
        );
    }
    hits.extend(
        content_ids(&state, &compiled)
            .await?
            .into_iter()
            .map(|id| (false, id)),
    );
    let total = hits.len() as i64;
    let expand = params.expand();
    let mut ctx = WikiCtx::new(&state);
    let mut results = Vec::new();
    for (is_space, id) in hits.into_iter().skip(start as usize).take(limit as usize) {
        if is_space {
            let Some(space) = spaces::by_id(&state.db, id).await? else {
                continue;
            };
            results.push(json!({
                "space": ctx.space(&space, &expand.below("space")).await?,
                "title": space.name,
                "excerpt": space.description,
                "url": format!("/spaces/{}", space.key),
                "resultGlobalContainer": { "title": space.name, "displayUrl": format!("/spaces/{}", space.key) },
                "breadcrumbs": [],
                "entityType": "space",
                "iconCssClass": "aui-icon content-type-space",
                "lastModified": format_iso(space.created_at),
                "friendlyLastModified": format_iso(space.created_at),
                "score": 0.0,
            }));
            continue;
        }
        let Some(found) = content::by_id(&state.db, id).await? else {
            continue;
        };
        let space = spaces::by_id(&state.db, found.space_id).await?;
        let (space_name, space_key) = space.map(|s| (s.name, s.key)).unwrap_or_default();
        let content_json = ctx.content(&found, &expand.below("content")).await?;
        let url = content_json["_links"]["webui"].clone();
        results.push(json!({
            "content": content_json,
            "title": found.title,
            "excerpt": excerpt(&found.body),
            "url": url,
            "resultGlobalContainer": { "title": space_name, "displayUrl": format!("/spaces/{space_key}") },
            "breadcrumbs": [],
            "entityType": "content",
            "iconCssClass": format!("aui-icon content-type-{}", found.content_type),
            "lastModified": format_iso(found.updated_at),
            "friendlyLastModified": format_iso(found.updated_at),
            "score": 0.0,
        }));
    }
    let size = results.len();
    Ok(Json(json!({
        "results": results,
        "start": start,
        "limit": limit,
        "size": size,
        "totalSize": total,
        "cqlQuery": cql_text,
        "searchDuration": 0,
        "_links": { "base": ctx.base(), "context": "/wiki" },
    })))
}
