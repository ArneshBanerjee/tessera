//! Issues: create, read, edit, delete, assign, history, and the create and edit metadata.

use crate::api::access::Access;
use crate::api::access::find_issue_as;
use std::collections::BTreeMap;

use super::extract::Json;
use axum::Extension;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::fields::{SYSTEM_FIELDS, custom_field_json, custom_field_key, system_field_json};
use super::issue_fields::{self, CustomCatalog, Mode, render_custom_value};
use super::render::Ctx;
use super::{ApiVersion, Page};
use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::store::changelog::{self, Changelog};
use crate::store::issues::{self, Issue, IssueFields};
use crate::store::social::{self, List};
use crate::store::{comments, projects, workflows};
use crate::time::{format, now_ms};

/// Which fields and expansions to include when rendering an issue.
#[derive(Debug, Clone, Default)]
pub struct IssueView {
    /// Field ids to include. Empty means all fields.
    pub include: Vec<String>,
    /// Field ids to leave out (`-field` in the request).
    pub exclude: Vec<String>,
    pub expand: Vec<String>,
}

impl IssueView {
    /// Reads `fields` and `expand` from a query string. Both may be comma-separated or repeated.
    pub fn from_query(raw: Option<&str>) -> Self {
        let fields = query_list(raw, "fields");
        Self::new(&fields, &query_list(raw, "expand"))
    }

    pub fn new(fields: &[String], expand: &[String]) -> Self {
        let mut view = Self {
            expand: expand.to_vec(),
            ..Self::default()
        };
        for field in fields {
            match field.as_str() {
                // Tessera treats navigable fields and all fields the same.
                "*all" | "*navigable" => {}
                f if f.starts_with('-') => view.exclude.push(f[1..].to_string()),
                f => view.include.push(f.to_string()),
            }
        }
        view
    }

    fn wants(&self, field: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|f| f == field))
            && !self.exclude.iter().any(|f| f == field)
    }

    fn expands(&self, name: &str) -> bool {
        self.expand.iter().any(|e| e == name)
    }
}

/// All values of a query parameter, splitting comma-separated lists.
pub fn query_list(raw: Option<&str>, name: &str) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    form_urlencoded::parse(raw.as_bytes())
        .filter(|(key, _)| key == name)
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A short issue reference with a few fields, as used for `parent` and `subtasks`.
fn issue_ref(ctx: &Ctx, issue: &Issue) -> Value {
    let catalog = &ctx.catalog;
    json!({
        "id": issue.id.to_string(),
        "key": issue.key,
        "self": ctx.api_url(&format!("/issue/{}", issue.id)),
        "fields": {
            "summary": issue.summary,
            "status": catalog.status(issue.status_id).map(|s| ctx.status(s)),
            "priority": issue.priority_id.and_then(|id| catalog.priority(id)).map(|p| ctx.priority(p)),
            "issuetype": catalog.issue_type(issue.issue_type_id).map(|t| ctx.issue_type(t)),
        },
    })
}

pub async fn comment_json(
    ctx: &mut Ctx,
    issue_id: i64,
    comment: &comments::Comment,
) -> ApiResult<Value> {
    Ok(json!({
        "self": ctx.api_url(&format!("/issue/{issue_id}/comment/{}", comment.id)),
        "id": comment.id.to_string(),
        "author": ctx.user_ref(Some(comment.author_id)).await?,
        "body": ctx.rich_text(Some(&comment.body)),
        "updateAuthor": ctx.user_ref(Some(comment.update_author_id)).await?,
        "created": format(comment.created_at),
        "updated": format(comment.updated_at),
        "jsdPublic": true,
    }))
}

async fn history_json(ctx: &mut Ctx, entry: &Changelog) -> ApiResult<Value> {
    let items: Vec<Value> = entry
        .items
        .iter()
        .map(|item| {
            json!({
                "field": item.field,
                "fieldtype": item.field_type,
                "fieldId": item.field_id,
                "from": item.from_value,
                "fromString": item.from_string,
                "to": item.to_value,
                "toString": item.to_string,
            })
        })
        .collect();
    Ok(json!({
        "id": entry.id.to_string(),
        "author": ctx.user_ref(Some(entry.author_id)).await?,
        "created": format(entry.created_at),
        "items": items,
    }))
}

/// Field id to display name, for `expand=names`.
fn names(custom: &CustomCatalog) -> Map<String, Value> {
    let mut names: Map<String, Value> = SYSTEM_FIELDS
        .iter()
        .map(|f| (f.id.to_string(), json!(f.name)))
        .collect();
    for field in &custom.fields {
        names.insert(custom_field_key(field.id), json!(field.name));
    }
    names
}

/// Adds a field to an issue's `fields` if the view asks for it.
fn put(fields: &mut Map<String, Value>, view: &IssueView, name: &str, value: Value) {
    if view.wants(name) {
        fields.insert(name.to_string(), value);
    }
}

/// Renders an issue in the REST API's shape.
pub async fn issue_json(
    ctx: &mut Ctx,
    custom: &CustomCatalog,
    issue: &Issue,
    view: &IssueView,
) -> ApiResult<Value> {
    let db = ctx.state.db.clone();
    let project = projects::by_id(&db, issue.project_id)
        .await?
        .expect("issues always have a project");
    let catalog = ctx.catalog.clone();
    let mut fields = Map::new();
    let f = &mut fields;

    put(f, view, "summary", json!(issue.summary));
    put(
        f,
        view,
        "description",
        ctx.rich_text(issue.description.as_deref()),
    );
    put(
        f,
        view,
        "environment",
        ctx.rich_text(issue.environment.as_deref()),
    );
    put(
        f,
        view,
        "issuetype",
        json!(
            catalog
                .issue_type(issue.issue_type_id)
                .map(|t| ctx.issue_type(t))
        ),
    );
    put(f, view, "project", ctx.project_ref(&project));
    let status = catalog.status(issue.status_id);
    put(f, view, "status", json!(status.map(|s| ctx.status(s))));
    put(
        f,
        view,
        "statusCategory",
        json!(
            status
                .and_then(|s| catalog.status_category(s.category_id))
                .map(|c| ctx.status_category(c))
        ),
    );
    put(
        f,
        view,
        "priority",
        json!(
            issue
                .priority_id
                .and_then(|id| catalog.priority(id))
                .map(|p| ctx.priority(p))
        ),
    );
    put(
        f,
        view,
        "resolution",
        json!(
            issue
                .resolution_id
                .and_then(|id| catalog.resolution(id))
                .map(|r| ctx.resolution(r))
        ),
    );
    put(
        f,
        view,
        "resolutiondate",
        json!(issue.resolution_date.map(format)),
    );
    put(f, view, "duedate", json!(issue.due_date));
    put(f, view, "created", json!(format(issue.created_at)));
    put(f, view, "updated", json!(format(issue.updated_at)));
    put(
        f,
        view,
        "labels",
        json!(issues::labels(&db, issue.id).await?),
    );

    put(f, view, "assignee", ctx.user_ref(issue.assignee_id).await?);
    put(f, view, "reporter", ctx.user_ref(issue.reporter_id).await?);
    put(
        f,
        view,
        "creator",
        ctx.user_ref(Some(issue.creator_id)).await?,
    );

    if view.wants("parent")
        && let Some(parent_id) = issue.parent_id
        && let Some(parent) = issues::by_id(&db, parent_id).await?
    {
        fields.insert("parent".into(), issue_ref(ctx, &parent));
    }
    if view.wants("subtasks") {
        let subtasks = issues::subtasks(&db, issue.id).await?;
        let subtask_type = |i: &Issue| {
            catalog
                .issue_type(i.issue_type_id)
                .is_some_and(|t| t.is_subtask())
        };
        fields.insert(
            "subtasks".into(),
            Value::Array(
                subtasks
                    .iter()
                    .filter(|i| subtask_type(i))
                    .map(|i| issue_ref(ctx, i))
                    .collect(),
            ),
        );
    }
    if view.wants("comment") {
        let (page, total) = comments::list(&db, issue.id, 0, 5000).await?;
        let mut rendered = Vec::with_capacity(page.len());
        for comment in &page {
            rendered.push(comment_json(ctx, issue.id, comment).await?);
        }
        fields.insert(
            "comment".into(),
            json!({
                "comments": rendered,
                "self": ctx.api_url(&format!("/issue/{}/comment", issue.id)),
                "maxResults": total,
                "total": total,
                "startAt": 0,
            }),
        );
    }

    if ["components", "fixVersions", "versions"]
        .iter()
        .any(|f| view.wants(f))
    {
        let (components, fix_versions, affects_versions) =
            super::parts::issue_parts_json(ctx, issue.id).await?;
        let f = &mut fields;
        put(f, view, "components", components);
        put(f, view, "fixVersions", fix_versions);
        put(f, view, "versions", affects_versions);
    }
    if view.wants("issuelinks") {
        fields.insert(
            "issuelinks".into(),
            super::links::issue_links_json(ctx, issue).await?,
        );
    }
    if view.wants("attachment") {
        fields.insert(
            "attachment".into(),
            super::attachments::issue_attachments_json(ctx, issue.id).await?,
        );
    }
    if view.wants("watches") {
        fields.insert(
            "watches".into(),
            super::social::watches_json(ctx, issue).await?,
        );
    }
    if view.wants("votes") {
        fields.insert("votes".into(), super::social::votes_json(ctx, issue).await?);
    }
    if view.wants("worklog") {
        fields.insert(
            "worklog".into(),
            super::worklogs::issue_worklog_json(ctx, issue.id).await?,
        );
    }
    time_tracking_fields(&db, issue, view, &mut fields).await?;

    let stored = issues::custom_values(&db, issue.id).await?;
    for field in &custom.fields {
        let key = custom_field_key(field.id);
        if !view.wants(&key) {
            continue;
        }
        let value = match field.field_type.as_str() {
            // Jira Software fields keep their values outside the custom value table.
            "sprint" => {
                let all = crate::store::sprints::of_issue(&db, issue.id).await?;
                if all.is_empty() {
                    Value::Null
                } else {
                    Value::Array(
                        all.iter()
                            .map(|s| super::agile::sprint_json(ctx, s))
                            .collect(),
                    )
                }
            }
            "rank" => crate::store::ranks::of_issue(&db, issue.id)
                .await?
                .map(|rank| json!(crate::store::ranks::display(&rank)))
                .unwrap_or(Value::Null),
            _ => match stored.iter().find(|(id, _)| *id == field.id) {
                Some((_, value)) => render_custom_value(ctx, custom, field, value).await?,
                None => Value::Null,
            },
        };
        fields.insert(key, value);
    }

    let mut result = json!({
        "expand": "renderedFields,names,schema,operations,editmeta,changelog,versionedRepresentations",
        "id": issue.id.to_string(),
        "self": ctx.api_url(&format!("/issue/{}", issue.id)),
        "key": issue.key,
        "fields": fields,
    });
    if view.expands("names") {
        result["names"] = Value::Object(names(custom));
    }
    if view.expands("renderedFields") {
        result["renderedFields"] = json!({
            "description": issue.description.as_deref().map(render_html),
            "environment": issue.environment.as_deref().map(render_html),
            "created": format(issue.created_at),
            "updated": format(issue.updated_at),
        });
    }
    if view.expands("transitions") {
        result["transitions"] =
            json!(super::transitions::transitions_json(ctx, &project, issue).await?);
    }
    if view.expands("changelog") {
        let histories = changelog::for_issue(&db, issue.id).await?;
        let mut rendered = Vec::with_capacity(histories.len());
        for entry in &histories {
            rendered.push(history_json(ctx, entry).await?);
        }
        result["changelog"] = json!({
            "startAt": 0,
            "maxResults": rendered.len(),
            "total": rendered.len(),
            "histories": rendered,
        });
    }
    Ok(result)
}

/// Time tracking fields: estimates, time spent, their totals with subtasks, and progress.
async fn time_tracking_fields(
    db: &sqlx::SqlitePool,
    issue: &Issue,
    view: &IssueView,
    fields: &mut Map<String, Value>,
) -> ApiResult<()> {
    let spent = crate::store::worklogs::time_spent(db, issue.id).await?;
    let (original, remaining) = (issue.original_estimate, issue.remaining_estimate);
    // Aggregates add up the issue and its subtasks.
    let mut aggregate = (original, remaining, spent);
    for subtask in issues::subtasks(db, issue.id).await? {
        let add = |total: Option<i64>, part: Option<i64>| match (total, part) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        aggregate.0 = add(aggregate.0, subtask.original_estimate);
        aggregate.1 = add(aggregate.1, subtask.remaining_estimate);
        aggregate.2 += crate::store::worklogs::time_spent(db, subtask.id).await?;
    }
    let nonzero = |seconds: i64| (seconds > 0).then_some(seconds);
    let progress = |spent: i64, remaining: Option<i64>| {
        let total = spent + remaining.unwrap_or(0);
        let mut value = json!({ "progress": spent, "total": total });
        if total > 0 {
            value["percent"] = json!(spent * 100 / total);
        }
        value
    };
    let mut tracking = Map::new();
    if let Some(seconds) = original {
        tracking.insert(
            "originalEstimate".into(),
            json!(crate::time::format_duration(seconds)),
        );
        tracking.insert("originalEstimateSeconds".into(), json!(seconds));
    }
    if let Some(seconds) = remaining {
        tracking.insert(
            "remainingEstimate".into(),
            json!(crate::time::format_duration(seconds)),
        );
        tracking.insert("remainingEstimateSeconds".into(), json!(seconds));
    }
    if spent > 0 {
        tracking.insert(
            "timeSpent".into(),
            json!(crate::time::format_duration(spent)),
        );
        tracking.insert("timeSpentSeconds".into(), json!(spent));
    }
    let f = fields;
    put(f, view, "timetracking", Value::Object(tracking));
    put(f, view, "timeoriginalestimate", json!(original));
    put(f, view, "timeestimate", json!(remaining));
    put(f, view, "timespent", json!(nonzero(spent)));
    put(f, view, "aggregatetimeoriginalestimate", json!(aggregate.0));
    put(f, view, "aggregatetimeestimate", json!(aggregate.1));
    put(f, view, "aggregatetimespent", json!(nonzero(aggregate.2)));
    put(f, view, "progress", progress(spent, remaining));
    put(
        f,
        view,
        "aggregateprogress",
        progress(aggregate.2, aggregate.1),
    );
    let ratio = match original {
        Some(original) if original > 0 => spent * 100 / original,
        _ => -1,
    };
    put(f, view, "workratio", json!(ratio));
    Ok(())
}

/// A simple HTML rendering of rich text, for `renderedFields`.
fn render_html(adf_json: &str) -> String {
    let text = serde_json::from_str::<tessera_adf::Node>(adf_json)
        .map(|doc| tessera_adf::to_plain_text(&doc))
        .unwrap_or_default();
    text.split("\n\n")
        .map(|paragraph| {
            let escaped = paragraph
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('\n', "<br/>");
            format!("<p>{escaped}</p>")
        })
        .collect()
}

pub async fn find_issue(state: &AppState, id_or_key: &str) -> ApiResult<Issue> {
    issues::by_id_or_key(&state.db, id_or_key)
        .await?
        .ok_or_else(ApiError::issue_not_found)
}

pub async fn get(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let mut ctx = Ctx::new(&state, version).await?.viewed_by(user.id);
    let custom = CustomCatalog::load(&state.db).await?;
    let view = IssueView::from_query(raw.as_deref());
    Ok(Json(issue_json(&mut ctx, &custom, &issue, &view).await?))
}

#[derive(Deserialize, Default)]
pub struct IssueRequest {
    #[serde(default)]
    pub fields: Map<String, Value>,
    #[serde(default)]
    pub update: Map<String, Value>,
}

/// Keeps the resolution date in step with the resolution, as Jira does.
fn sync_resolution_date(before: Option<i64>, fields: &mut IssueFields) {
    match (before, fields.resolution_id) {
        (None, Some(_)) => fields.resolution_date = Some(now_ms()),
        (_, None) => fields.resolution_date = None,
        _ => {}
    }
}

pub async fn create(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<IssueRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let project_ref = request
        .fields
        .get("project")
        .cloned()
        .unwrap_or(Value::Null);
    let project_id_or_key = project_ref
        .get("key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            project_ref
                .get("id")
                .and_then(crate::store::meta::json_id)
                .map(|id| id.to_string())
        })
        .ok_or_else(|| ApiError::field("project", "Specify a valid project ID or key"))?;
    let project = projects::by_id_or_key(&state.db, &project_id_or_key)
        .await?
        .ok_or_else(|| ApiError::field("project", "Specify a valid project ID or key"))?;
    // A project the user cannot see is reported the same way as one that does not exist.
    let access = Access::load(&state.db, &user, &project).await?;
    if !access.can("BROWSE_PROJECTS") {
        return Err(ApiError::field(
            "project",
            "Specify a valid project ID or key",
        ));
    }
    if !access.can("CREATE_ISSUES") {
        return Err(ApiError::field(
            "project",
            "You do not have permission to create issues in this project.",
        ));
    }
    if !request.fields.contains_key("issuetype") {
        return Err(ApiError::field("issuetype", "Specify an issue type"));
    }

    let mut ctx = Ctx::new(&state, version).await?;
    let custom = CustomCatalog::load(&state.db).await?;
    let mut fields = IssueFields {
        status_id: workflows::initial_status(&state.db, project.workflow_id).await?,
        priority_id: ctx.catalog.default_priority(),
        reporter_id: Some(user.id),
        ..IssueFields::default()
    };
    let new_comments = issue_fields::apply(
        &mut ctx,
        &custom,
        &project,
        &mut fields,
        &request.fields,
        &request.update,
        Mode::Create,
    )
    .await?;
    sync_resolution_date(None, &mut fields);
    // No assignee in the request means "automatic": the component or project default.
    if !request.fields.contains_key("assignee") {
        fields.assignee_id =
            super::parts::default_assignee(&state.db, &project, &fields.components).await?;
    }
    let issue = issues::create(&state.db, project.id, user.id, &fields).await?;
    // The creator watches the new issue, as with Jira's default autowatch setting.
    social::add(&state.db, List::Watchers, issue.id, user.id).await?;
    for body in &new_comments {
        comments::create(&state.db, issue.id, user.id, body).await?;
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": issue.id.to_string(),
            "key": issue.key,
            "self": ctx.api_url(&format!("/issue/{}", issue.id)),
        })),
    ))
}

/// Applies an edit to an issue and records the changes in its history.
pub async fn save_edit(
    ctx: &mut Ctx,
    issue: &Issue,
    author_id: i64,
    request: &IssueRequest,
) -> ApiResult<()> {
    let db = ctx.state.db.clone();
    let project = projects::by_id(&db, issue.project_id)
        .await?
        .expect("issue has a project");
    let custom = CustomCatalog::load(&db).await?;
    let before_custom = issues::custom_values(&db, issue.id).await?;
    let before = IssueFields::load(&db, issue).await?;
    let mut after = before.clone();
    after.custom_values = before_custom
        .iter()
        .map(|(id, v)| (*id, Some(v.clone())))
        .collect();
    let new_comments = issue_fields::apply(
        ctx,
        &custom,
        &project,
        &mut after,
        &request.fields,
        &request.update,
        Mode::Edit,
    )
    .await?;
    sync_resolution_date(before.resolution_id, &mut after);
    let changes = issue_fields::diff(ctx, &custom, &before, &before_custom, &after).await?;
    issues::update(&db, issue.id, author_id, &after, &changes).await?;
    for body in &new_comments {
        comments::create(&db, issue.id, author_id, body).await?;
        social::add(&db, List::Watchers, issue.id, author_id).await?;
    }
    Ok(())
}

pub async fn edit(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(request): Json<IssueRequest>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "EDIT_ISSUES").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    save_edit(&mut ctx, &issue, user.id, &request).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuery {
    /// Jira parses booleans case-insensitively; Python clients send `True` and `False`.
    #[serde(default)]
    delete_subtasks: Option<String>,
}

pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "DELETE_ISSUES").await?;
    let catalog = crate::store::meta::Catalog::load(&state.db).await?;
    let subtasks: Vec<Issue> = issues::subtasks(&state.db, issue.id)
        .await?
        .into_iter()
        .filter(|i| {
            catalog
                .issue_type(i.issue_type_id)
                .is_some_and(|t| t.is_subtask())
        })
        .collect();
    let delete_subtasks = query
        .delete_subtasks
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if !subtasks.is_empty() && !delete_subtasks {
        return Err(ApiError::bad_request(
            "The issue has subtasks. Use the parameter deleteSubtasks=true to delete the issue and its subtasks.",
        ));
    }
    for subtask in subtasks {
        issues::delete(&state.db, subtask.id).await?;
    }
    issues::delete(&state.db, issue.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /issue/{idOrKey}/assignee` with `{"accountId": ...}`. `null` unassigns.
pub async fn assign(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Json(assignee): Json<Value>,
) -> ApiResult<StatusCode> {
    let issue = find_issue_as(&state, &user, &id_or_key, "ASSIGN_ISSUES").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let request = IssueRequest {
        fields: Map::from_iter([("assignee".to_string(), assignee)]),
        update: Map::new(),
    };
    save_edit(&mut ctx, &issue, user.id, &request).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn changelog(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let issue = find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let mut ctx = Ctx::new(&state, version).await?;
    let all = changelog::for_issue(&state.db, issue.id).await?;
    let (start_at, max_results) = page.bounded(100);
    let total = all.len() as i64;
    let mut values = Vec::new();
    for entry in all
        .iter()
        .skip(start_at as usize)
        .take(max_results as usize)
    {
        values.push(history_json(&mut ctx, entry).await?);
    }
    Ok(Json(json!({
        "self": ctx.api_url(&format!("/issue/{}/changelog?startAt={start_at}&maxResults={max_results}", issue.key)),
        "maxResults": max_results,
        "startAt": start_at,
        "total": total,
        "isLast": start_at + max_results >= total,
        "values": values,
    })))
}

/// Field metadata for create and edit screens: which fields exist, their types, and their
/// allowed values. Tessera puts every settable field on every screen.
fn field_meta(ctx: &Ctx, custom: &CustomCatalog, for_create: bool) -> BTreeMap<String, Value> {
    let mut meta = BTreeMap::new();
    let refs = |items: Vec<Value>| json!(items);
    for field in SYSTEM_FIELDS.iter().filter(|f| f.settable) {
        if !for_create && field.id == "project" {
            continue;
        }
        let mut entry = system_field_json(field);
        let entry_obj = entry.as_object_mut().expect("object");
        entry_obj.retain(|k, _| matches!(k.as_str(), "key" | "name" | "schema"));
        entry_obj.insert("fieldId".into(), json!(field.id));
        entry_obj.insert(
            "required".into(),
            json!(for_create && matches!(field.id, "summary" | "issuetype" | "project")),
        );
        entry_obj.insert(
            "hasDefaultValue".into(),
            json!(matches!(field.id, "priority")),
        );
        let operations = match field.id {
            "labels" | "components" | "fixVersions" | "versions" => json!(["add", "set", "remove"]),
            _ => json!(["set"]),
        };
        entry_obj.insert("operations".into(), operations);
        let allowed = match field.id {
            "issuetype" => Some(refs(
                ctx.catalog
                    .issue_types
                    .iter()
                    .map(|t| ctx.issue_type(t))
                    .collect(),
            )),
            "priority" => Some(refs(
                ctx.catalog
                    .priorities
                    .iter()
                    .map(|p| ctx.priority(p))
                    .collect(),
            )),
            "resolution" => Some(refs(
                ctx.catalog
                    .resolutions
                    .iter()
                    .map(|r| ctx.resolution(r))
                    .collect(),
            )),
            _ => None,
        };
        if let Some(allowed) = allowed {
            entry_obj.insert("allowedValues".into(), allowed);
        }
        meta.insert(field.id.to_string(), entry);
    }
    for field in &custom.fields {
        let mut entry = custom_field_json(field);
        let entry_obj = entry.as_object_mut().expect("object");
        entry_obj.retain(|k, _| matches!(k.as_str(), "key" | "name" | "schema"));
        let key = custom_field_key(field.id);
        entry_obj.insert("fieldId".into(), json!(key));
        entry_obj.insert("required".into(), json!(false));
        entry_obj.insert("hasDefaultValue".into(), json!(false));
        let operations = match field.field_type.as_str() {
            "labels" | "options" => json!(["add", "set", "remove"]),
            _ => json!(["set"]),
        };
        entry_obj.insert("operations".into(), operations);
        if matches!(field.field_type.as_str(), "option" | "options") {
            let options: Vec<Value> = custom
                .options
                .iter()
                .filter(|o| o.field_id == field.id && !o.disabled)
                .map(|o| {
                    json!({
                        "self": ctx.api_url(&format!("/customFieldOption/{}", o.id)),
                        "value": o.value,
                        "id": o.id.to_string(),
                    })
                })
                .collect();
            entry_obj.insert("allowedValues".into(), json!(options));
        }
        meta.insert(key, entry);
    }
    meta
}

/// The legacy `GET /issue/createmeta`, which many clients still use.
pub async fn create_meta(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let raw = raw.as_deref();
    let ctx = Ctx::new(&state, version).await?;
    let custom = CustomCatalog::load(&state.db).await?;
    let keys = query_list(raw, "projectKeys");
    let ids = query_list(raw, "projectIds");
    let type_names = query_list(raw, "issuetypeNames");
    let type_ids = query_list(raw, "issuetypeIds");
    let with_fields = query_list(raw, "expand")
        .iter()
        .any(|e| e == "projects.issuetypes.fields");
    let fields = field_meta(&ctx, &custom, true);

    let mut projects_json = Vec::new();
    for project in projects::list(&state.db).await? {
        let selected = (keys.is_empty() && ids.is_empty())
            || keys.iter().any(|k| k.eq_ignore_ascii_case(&project.key))
            || ids.contains(&project.id.to_string());
        // Only projects the user may create issues in, as in Jira.
        if !selected
            || !Access::load(&state.db, &user, &project)
                .await?
                .can("CREATE_ISSUES")
        {
            continue;
        }
        let issue_types: Vec<Value> = ctx
            .catalog
            .issue_types
            .iter()
            .filter(|t| {
                (type_names.is_empty() && type_ids.is_empty())
                    || type_names.iter().any(|n| n.eq_ignore_ascii_case(&t.name))
                    || type_ids.contains(&t.id.to_string())
            })
            .map(|t| {
                let mut value = ctx.issue_type(t);
                if with_fields {
                    value["fields"] = json!(fields);
                }
                value
            })
            .collect();
        let mut value = ctx.project_ref(&project);
        value["issuetypes"] = json!(issue_types);
        projects_json.push(value);
    }
    Ok(Json(
        json!({ "expand": "projects", "projects": projects_json }),
    ))
}

pub async fn create_meta_issue_types(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path(project): Path<String>,
) -> ApiResult<Json<Value>> {
    projects::by_id_or_key(&state.db, &project)
        .await?
        .ok_or_else(|| ApiError::project_not_found(&project))?;
    let ctx = Ctx::new(&state, version).await?;
    let types: Vec<Value> = ctx
        .catalog
        .issue_types
        .iter()
        .map(|t| ctx.issue_type(t))
        .collect();
    let total = types.len();
    Ok(Json(json!({
        "maxResults": 50,
        "startAt": 0,
        "total": total,
        "issueTypes": types,
        "createMetaIssueType": types,
    })))
}

pub async fn create_meta_fields(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(_): CurrentUser,
    Path((project, issue_type)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    projects::by_id_or_key(&state.db, &project)
        .await?
        .ok_or_else(|| ApiError::project_not_found(&project))?;
    let ctx = Ctx::new(&state, version).await?;
    issue_type
        .parse()
        .ok()
        .and_then(|id| ctx.catalog.issue_type(id))
        .ok_or_else(|| ApiError::not_found("The issue type was not found."))?;
    let custom = CustomCatalog::load(&state.db).await?;
    let fields: Vec<Value> = field_meta(&ctx, &custom, true).into_values().collect();
    let total = fields.len();
    Ok(Json(json!({
        "maxResults": 50,
        "startAt": 0,
        "total": total,
        "fields": fields,
        "results": fields,
    })))
}

pub async fn edit_meta(
    State(state): State<AppState>,
    Extension(version): Extension<ApiVersion>,
    CurrentUser(user): CurrentUser,
    Path(id_or_key): Path<String>,
) -> ApiResult<Json<Value>> {
    find_issue_as(&state, &user, &id_or_key, "BROWSE_PROJECTS").await?;
    let ctx = Ctx::new(&state, version).await?;
    let custom = CustomCatalog::load(&state.db).await?;
    Ok(Json(json!({ "fields": field_meta(&ctx, &custom, false) })))
}
