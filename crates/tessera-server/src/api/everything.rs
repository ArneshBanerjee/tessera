//! Search across Jira and Confluence: `GET /rest/tessera/1.0/search?query=`, which the web
//! app's search box uses, as Atlassian's own search across products does.
//!
//! Results come in sections: issues, pages and blog posts, spaces, projects, filters, and
//! dashboards. Each section holds only what the user may see.

use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::access::{Access, find_issue_as};
use super::extract::Json;
use super::render::icon_slug;
use super::search::run_jql;
use crate::AppState;
use crate::auth::CurrentUser;
use crate::confluence;
use crate::error::ApiResult;
use crate::notify::content_link;
use crate::store::issues::{self, Issue};
use crate::store::meta::Catalog;
use crate::store::users::User;
use crate::store::{projects, spaces};
use crate::time::format;

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    query: String,
    /// Results per section.
    limit: Option<usize>,
}

/// Text as a quoted JQL or CQL string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn section(key: &str, label: &str, results: Vec<Value>, more: Option<String>) -> Value {
    json!({ "key": key, "label": label, "results": results, "more": more })
}

/// The issue whose key the query is, if the user can see it.
async fn by_key(state: &AppState, user: &User, query: &str) -> Option<Issue> {
    let (project, number) = query.trim().rsplit_once('-')?;
    if project.is_empty() || number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    find_issue_as(state, user, query.trim(), "BROWSE_PROJECTS")
        .await
        .ok()
}

async fn issue_results(
    state: &AppState,
    user: &User,
    query: &str,
    limit: usize,
) -> ApiResult<(Vec<Value>, String)> {
    let jql = format!("text ~ {} ORDER BY updated DESC", quoted(query));
    let mut found: Vec<Issue> = Vec::new();
    if let Some(issue) = by_key(state, user, query).await {
        found.push(issue);
    }
    // Text that JQL cannot parse finds nothing here, rather than failing the whole search.
    if let Ok((ids, _)) = run_jql(&state.db, Some(user), &jql, 0, limit as i64).await {
        for issue in issues::by_ids(&state.db, &ids).await? {
            if !found.iter().any(|f| f.id == issue.id) {
                found.push(issue);
            }
        }
    }
    found.truncate(limit);
    let catalog = Catalog::load(&state.db).await?;
    let mut out = Vec::new();
    for issue in found {
        let project = projects::by_id(&state.db, issue.project_id).await?;
        let status = catalog.status(issue.status_id).map(|s| s.name.clone());
        let kind = catalog.issue_type(issue.issue_type_id);
        let subtitle: Vec<String> = [project.map(|p| p.name), status]
            .into_iter()
            .flatten()
            .collect();
        out.push(json!({
            "type": "issue",
            "title": format!("{} {}", issue.key, issue.summary),
            "subtitle": subtitle.join(" · "),
            "link": format!("/browse/{}", issue.key),
            "icon": kind.map(|t| format!("/icons/issuetype-{}.svg", icon_slug(&t.name))),
            "updated": format(issue.updated_at),
        }));
    }
    Ok((
        out,
        format!(
            "/issues?jql={}",
            form_urlencoded::byte_serialize(jql.as_bytes()).collect::<String>()
        ),
    ))
}

async fn content_results(
    state: &AppState,
    user: &User,
    query: &str,
    limit: usize,
) -> ApiResult<Vec<Value>> {
    let text = quoted(query);
    let cql = format!(
        "type in (page, blogpost) AND (title ~ {text} OR text ~ {text}) ORDER BY lastmodified DESC"
    );
    let Ok(found) = confluence::search::find_content(state, user, &cql).await else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for content in found.into_iter().take(limit) {
        let Some(space) = spaces::by_id(&state.db, content.space_id).await? else {
            continue;
        };
        let kind = if content.content_type == "blogpost" {
            "Blog post"
        } else {
            "Page"
        };
        out.push(json!({
            "type": content.content_type,
            "title": content.title,
            "subtitle": format!("{kind} · {}", space.name),
            "link": content_link(&space.key, &content),
            "updated": format(content.updated_at),
        }));
    }
    Ok(out)
}

async fn space_results(
    state: &AppState,
    user: &User,
    query: &str,
    limit: usize,
) -> ApiResult<Vec<Value>> {
    let text = quoted(query);
    let cql = format!("title ~ {text} OR space.key = {text}");
    let Ok(found) = confluence::search::find_spaces(state, user, &cql).await else {
        return Ok(Vec::new());
    };
    Ok(found
        .into_iter()
        .take(limit)
        .map(|space| {
            json!({
                "type": "space",
                "title": space.name,
                "subtitle": format!("Space · {}", space.key),
                "link": format!("/wiki/spaces/{}", space.key),
            })
        })
        .collect())
}

/// `GET /search?query=&limit=`.
pub async fn search(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let text = query.query.trim().to_string();
    let limit = query.limit.unwrap_or(5).clamp(1, 50);
    if text.is_empty() {
        return Ok(Json(json!({ "query": text, "sections": [] })));
    }
    let needle = text.to_lowercase();
    let mut sections = Vec::new();

    let (found_issues, issues_more) = issue_results(&state, &user, &text, limit).await?;
    sections.push(section("issues", "Issues", found_issues, Some(issues_more)));
    sections.push(section(
        "pages",
        "Pages and blog posts",
        content_results(&state, &user, &text, limit).await?,
        None,
    ));
    sections.push(section(
        "spaces",
        "Spaces",
        space_results(&state, &user, &text, limit).await?,
        None,
    ));

    let mut found_projects = Vec::new();
    for project in projects::list(&state.db).await? {
        let matches =
            project.name.to_lowercase().contains(&needle) || project.key.to_lowercase() == needle;
        if matches
            && Access::load(&state.db, &user, &project)
                .await?
                .can("BROWSE_PROJECTS")
        {
            found_projects.push(json!({
                "type": "project",
                "title": project.name,
                "subtitle": format!("Project · {}", project.key),
                "link": format!("/projects/{}", project.key),
            }));
        }
    }
    found_projects.truncate(limit);
    sections.push(section("projects", "Projects", found_projects, None));

    let found_filters: Vec<Value> = super::filters::visible(&state.db, Some(&user))
        .await?
        .into_iter()
        .filter(|f| f.name.to_lowercase().contains(&needle))
        .take(limit)
        .map(|f| {
            json!({
                "type": "filter",
                "title": f.name,
                "subtitle": "Filter",
                "link": format!("/issues?jql={}", form_urlencoded::byte_serialize(f.jql.as_bytes()).collect::<String>()),
            })
        })
        .collect();
    sections.push(section("filters", "Filters", found_filters, None));

    let found_dashboards: Vec<Value> = super::dashboards::visible(&state, &user)
        .await?
        .into_iter()
        .filter(|d| d.name.to_lowercase().contains(&needle))
        .take(limit)
        .map(|d| {
            json!({
                "type": "dashboard",
                "title": d.name,
                "subtitle": "Dashboard",
                "link": format!("/jira/dashboards/{}", d.id),
            })
        })
        .collect();
    sections.push(section("dashboards", "Dashboards", found_dashboards, None));

    Ok(Json(json!({ "query": text, "sections": sections })))
}
