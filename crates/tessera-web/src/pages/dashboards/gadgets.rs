//! Dashboard gadgets: the frame around each one, what each one shows, and its settings.
//!
//! Gadgets keep their settings in the `config` item property, with Jira's gadget preference
//! names: `filterId` and `num` for filter results, `projectOrFilterId` (`filter-10000` or
//! `project-KEY`), `statType`, and `daysprevious` for the charts.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde_json::{Value, json};

use crate::api::{self, ApiResult};
use crate::app::use_session;
use crate::components::{ErrorText, Lozenge, Modal, TypeIcon};
use crate::model::{Filter, Issue, Page, Project, SearchPage};
use crate::pages::reports::{millis, number, ticks};
use crate::pages::search;

use super::Gadget;

const INTRODUCTION: &str = "com.atlassian.jira.gadgets:introduction-dashboard-item";
const ASSIGNED: &str = "com.atlassian.jira.gadgets:assigned-to-me-dashboard-item";
const FILTER_RESULTS: &str = "com.atlassian.jira.gadgets:filter-results-dashboard-item";
const PIE_CHART: &str = "com.atlassian.jira.gadgets:pie-chart-dashboard-item";
const CREATED_RESOLVED: &str =
    "com.atlassian.jira.gadgets:created-vs-resolved-issues-chart-dashboard-item";
const WATCHED: &str = "com.atlassian.jira.gadgets:watched-issues-dashboard-item";

/// Pie chart groupings, as Jira names them: (statType, label).
const STAT_TYPES: &[(&str, &str)] = &[
    ("statuses", "Status"),
    ("assignees", "Assignee"),
    ("priorities", "Priority"),
    ("issuetype", "Issue type"),
    ("project", "Project"),
    ("reporter", "Reporter"),
    ("labels", "Labels"),
    ("resolution", "Resolution"),
];

fn configurable(key: &str) -> bool {
    matches!(key, FILTER_RESULTS | PIE_CHART | CREATED_RESOLVED)
}

fn gadget_path(dashboard: &str, gadget: i64) -> String {
    format!("{}/dashboard/{dashboard}/gadget/{gadget}", api::CORE)
}

fn config_path(dashboard: &str, gadget: i64) -> String {
    format!(
        "{}/dashboard/{dashboard}/items/{gadget}/properties/config",
        api::CORE
    )
}

/// JQL for a `projectOrFilterId` setting.
fn source_jql(source: &str) -> Option<String> {
    if let Some(id) = source.strip_prefix("filter-") {
        Some(format!("filter = {id}"))
    } else {
        source
            .strip_prefix("project-")
            .map(|key| format!("project = \"{key}\""))
    }
}

/// Every issue matching JQL, a thousand at a time.
async fn all_issues(jql: &str, fields: &[&str]) -> ApiResult<Vec<Issue>> {
    let mut out = Vec::new();
    loop {
        let page: SearchPage = api::post(
            &format!("{}/search", api::CORE),
            &json!({ "jql": jql, "startAt": out.len(), "maxResults": 1000, "fields": fields }),
        )
        .await?;
        let got = page.issues.len();
        out.extend(page.issues);
        if got == 0 || out.len() as i64 >= page.total {
            return Ok(out);
        }
    }
}

/// The card around a gadget, with its title and the controls to move, set up, and remove it.
#[component]
pub fn GadgetFrame(
    dashboard: String,
    gadget: Gadget,
    writable: bool,
    columns: i64,
    column_size: i64,
) -> impl IntoView {
    let session = use_session();
    let editing = RwSignal::new(false);
    let saved = RwSignal::new(0u32);
    let error = RwSignal::new(None::<String>);
    let key = gadget.module_key.clone().unwrap_or_default();
    let ids = StoredValue::new((dashboard.clone(), gadget.id));
    let has_settings = configurable(&key);
    let config = LocalResource::new(move || {
        saved.track();
        let (dashboard, id) = ids.get_value();
        async move {
            if !has_settings {
                return Value::Null;
            }
            api::get::<Value>(&config_path(&dashboard, id))
                .await
                .ok()
                .map(|p| p["value"].clone())
                .unwrap_or(Value::Null)
        }
    });
    let (col, row) = (gadget.position.column, gadget.position.row);
    let move_to = move |column: i64, row: i64| {
        let (dashboard, id) = ids.get_value();
        spawn_local(async move {
            let body = json!({ "position": { "column": column, "row": row } });
            match api::put::<Value>(&gadget_path(&dashboard, id), &body).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let remove = move |_| {
        let (dashboard, id) = ids.get_value();
        spawn_local(async move {
            match api::delete(&gadget_path(&dashboard, id)).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let save = Callback::new(move |value: Value| {
        let (dashboard, id) = ids.get_value();
        spawn_local(async move {
            match api::put::<Value>(&config_path(&dashboard, id), &value).await {
                Ok(_) => {
                    editing.set(false);
                    saved.update(|n| *n += 1);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    });
    let can_configure = writable && has_settings;
    let key = StoredValue::new(key);
    view! {
        <section class=format!("gadget gadget-{}", gadget.color)>
            <header class="gadget-header">
                <h2>{gadget.title.clone()}</h2>
                {writable.then(|| view! {
                    <div class="gadget-tools">
                        {can_configure.then(|| view! {
                            <button class="link-button small" on:click=move |_| editing.update(|e| *e = !*e)>"Configure"</button>
                        })}
                        <button class="icon-button" title="Move up" aria-label="Move up" disabled={row == 0}
                            on:click=move |_| move_to(col, row - 1)>"↑"</button>
                        <button class="icon-button" title="Move down" aria-label="Move down" disabled={row + 1 >= column_size}
                            on:click=move |_| move_to(col, row + 1)>"↓"</button>
                        <button class="icon-button" title="Move left" aria-label="Move left" disabled={col == 0}
                            on:click=move |_| move_to(col - 1, 0)>"←"</button>
                        <button class="icon-button" title="Move right" aria-label="Move right" disabled={col + 1 >= columns}
                            on:click=move |_| move_to(col + 1, 0)>"→"</button>
                        <button class="link-button small" on:click=remove>"Remove"</button>
                    </div>
                })}
            </header>
            <ErrorText error=error/>
            <div class="gadget-body">
                <Transition fallback=|| view! { <p class="muted small">"Loading…"</p> }>
                    {move || config.get().map(|config| {
                        if editing.get() {
                            view! { <ConfigForm kind=key.get_value() config=config on_save=save on_cancel=Callback::new(move |()| editing.set(false))/> }.into_any()
                        } else {
                            view! { <GadgetBody kind=key.get_value() config=config/> }.into_any()
                        }
                    })}
                </Transition>
            </div>
        </section>
    }
}

/// What a gadget shows.
#[component]
fn GadgetBody(kind: String, config: Value) -> impl IntoView {
    let source = config["projectOrFilterId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    match kind.as_str() {
        INTRODUCTION => view! { <Introduction/> }.into_any(),
        ASSIGNED => view! {
            <IssueList jql="assignee = currentUser() AND resolution = Unresolved ORDER BY priority DESC, created ASC".into() limit=10/>
        }
        .into_any(),
        WATCHED => view! {
            <IssueList jql="watcher = currentUser() AND resolution = Unresolved ORDER BY updated DESC".into() limit=10/>
        }
        .into_any(),
        FILTER_RESULTS => match config["filterId"].as_str() {
            Some(id) => {
                let limit = config["num"].as_i64().unwrap_or(10);
                view! { <IssueList jql=format!("filter = {id}") limit=limit/> }.into_any()
            }
            None => view! { <p class="muted small">"Configure this gadget to pick a filter."</p> }
                .into_any(),
        },
        PIE_CHART => match source_jql(&source) {
            Some(jql) => {
                let stat = config["statType"]
                    .as_str()
                    .unwrap_or("statuses")
                    .to_string();
                view! { <PieChart jql=jql stat=stat/> }.into_any()
            }
            None => view! { <p class="muted small">"Configure this gadget to pick a project or filter."</p> }
                .into_any(),
        },
        CREATED_RESOLVED => match source_jql(&source) {
            Some(jql) => {
                let days = config["daysprevious"].as_i64().unwrap_or(30);
                view! { <CreatedResolvedChart jql=jql days=days/> }.into_any()
            }
            None => view! { <p class="muted small">"Configure this gadget to pick a project or filter."</p> }
                .into_any(),
        },
        _ => view! { <p class="muted small">"Tessera cannot show this gadget."</p> }.into_any(),
    }
}

#[component]
fn Introduction() -> impl IntoView {
    view! {
        <div class="introduction">
            <p>"Welcome to Tessera. This dashboard collects the work that matters to you."</p>
            <ul>
                <li><A href="/projects">"Browse projects"</A></li>
                <li><A href="/issues">"Search for issues"</A></li>
                <li><A href="/jira/dashboards">"Make a dashboard of your own"</A></li>
            </ul>
        </div>
    }
}

/// A short list of issues from JQL, with a link to see them all.
#[component]
fn IssueList(jql: String, limit: i64) -> impl IntoView {
    let session = use_session();
    let query = StoredValue::new(jql);
    let results = LocalResource::new(move || {
        session.changes.track();
        async move { search(&query.get_value(), 0, limit).await }
    });
    view! {
        <Transition fallback=|| view! { <p class="muted small">"Loading…"</p> }>
            {move || results.get().map(|result| match result {
                Ok(page) if page.issues.is_empty() => view! { <p class="muted small">"No issues."</p> }.into_any(),
                Ok(page) => {
                    let total = page.total;
                    let shown = page.issues.len() as i64;
                    let all = format!("/issues?jql={}", api::encode(&query.get_value()));
                    view! {
                        <table class="gadget-table">
                            <tbody>
                                {page.issues.into_iter().map(|issue| {
                                    let href = format!("/browse/{}", issue.key);
                                    view! {
                                        <tr>
                                            <td class="icon-cell"><TypeIcon item=issue.fields.issuetype/></td>
                                            <td><A href=href.clone() attr:class="issue-key">{issue.key}</A></td>
                                            <td class="summary-cell"><A href=href attr:class="summary-link">{issue.fields.summary}</A></td>
                                            <td><Lozenge status=issue.fields.status/></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        <p class="muted small gadget-footer">
                            {format!("{shown} of {total} issues. ")}
                            <A href=all>"View all"</A>
                        </p>
                    }.into_any()
                }
                Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
            })}
        </Transition>
    }
}

/// The settings form of a configurable gadget.
#[component]
fn ConfigForm(
    kind: String,
    config: Value,
    on_save: Callback<Value>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let sources = LocalResource::new(|| async {
        let filters =
            api::get::<Page<Filter>>(&format!("{}/filter/search?maxResults=100", api::CORE))
                .await
                .map(|p| p.values)
                .unwrap_or_default();
        let projects = api::get::<Vec<Project>>(&format!("{}/project", api::CORE))
            .await
            .unwrap_or_default();
        (filters, projects)
    });
    let filter_only = kind == FILTER_RESULTS;
    let source = RwSignal::new(if filter_only {
        config["filterId"]
            .as_str()
            .map(|id| format!("filter-{id}"))
            .unwrap_or_default()
    } else {
        config["projectOrFilterId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    let stat = RwSignal::new(
        config["statType"]
            .as_str()
            .unwrap_or("statuses")
            .to_string(),
    );
    let days = RwSignal::new(config["daysprevious"].as_i64().unwrap_or(30));
    let num = RwSignal::new(config["num"].as_i64().unwrap_or(10));
    let error = RwSignal::new(None::<String>);
    let kind = StoredValue::new(kind);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let picked = source.get();
        if picked.is_empty() {
            error.set(Some("Pick a project or filter.".into()));
            return;
        }
        let value = match kind.get_value().as_str() {
            FILTER_RESULTS => json!({
                "filterId": picked.trim_start_matches("filter-"),
                "num": num.get(),
            }),
            PIE_CHART => json!({ "projectOrFilterId": picked, "statType": stat.get() }),
            _ => json!({ "projectOrFilterId": picked, "daysprevious": days.get() }),
        };
        on_save.run(value);
    };
    view! {
        <form class="form gadget-config" on:submit=submit>
            <label>{if filter_only { "Filter" } else { "Project or filter" }}
                <select class="select" on:change:target=move |ev| source.set(ev.target().value())>
                    <option value="" selected=move || source.get().is_empty()>"Pick one"</option>
                    {move || sources.get().map(|(filters, projects)| view! {
                        <optgroup label="Filters">
                            {filters.into_iter().map(|f| {
                                let value = format!("filter-{}", f.id);
                                let selected = source.get_untracked() == value;
                                view! { <option value=value selected=selected>{f.name}</option> }
                            }).collect_view()}
                        </optgroup>
                        {(!filter_only).then(|| view! {
                            <optgroup label="Projects">
                                {projects.into_iter().map(|p| {
                                    let value = format!("project-{}", p.key);
                                    let selected = source.get_untracked() == value;
                                    view! { <option value=value selected=selected>{p.name}</option> }
                                }).collect_view()}
                            </optgroup>
                        })}
                    })}
                </select>
            </label>
            {(kind.get_value() == PIE_CHART).then(|| view! {
                <label>"Group by"
                    <select class="select" on:change:target=move |ev| stat.set(ev.target().value())>
                        {STAT_TYPES.iter().map(|(value, label)| {
                            let selected = stat.get_untracked() == *value;
                            view! { <option value=*value selected=selected>{*label}</option> }
                        }).collect_view()}
                    </select>
                </label>
            })}
            {(kind.get_value() == CREATED_RESOLVED).then(|| view! {
                <label>"Days before today"
                    <select class="select" on:change:target=move |ev| days.set(ev.target().value().parse().unwrap_or(30))>
                        {[7, 14, 30, 60, 90].into_iter().map(|d| {
                            let selected = days.get_untracked() == d;
                            view! { <option value=d.to_string() selected=selected>{d.to_string()}</option> }
                        }).collect_view()}
                    </select>
                </label>
            })}
            {filter_only.then(|| view! {
                <label>"Number of results"
                    <select class="select" on:change:target=move |ev| num.set(ev.target().value().parse().unwrap_or(10))>
                        {[5, 10, 20, 50].into_iter().map(|n| {
                            let selected = num.get_untracked() == n;
                            view! { <option value=n.to_string() selected=selected>{n.to_string()}</option> }
                        }).collect_view()}
                    </select>
                </label>
            })}
            <ErrorText error=error/>
            <div class="form-actions">
                <button class="button primary" type="submit">"Save"</button>
                <button class="button subtle" type="button" on:click=move |_| on_cancel.run(())>"Cancel"</button>
            </div>
        </form>
    }
}

/// The names an issue counts under for a pie chart grouping.
fn categories(issue: &Issue, stat: &str) -> Vec<String> {
    let f = &issue.fields;
    let or = |value: Option<String>, none: &str| vec![value.unwrap_or_else(|| none.to_string())];
    match stat {
        "assignees" => or(
            f.assignee.as_ref().map(|u| u.display_name.clone()),
            "Unassigned",
        ),
        "priorities" => or(f.priority.as_ref().map(|p| p.name.clone()), "None"),
        "issuetype" => or(f.issuetype.as_ref().map(|t| t.name.clone()), "None"),
        "project" => or(f.project.as_ref().map(|p| p.name.clone()), "None"),
        "reporter" => or(
            f.reporter.as_ref().map(|u| u.display_name.clone()),
            "Anonymous",
        ),
        "labels" if f.labels.is_empty() => vec!["None".into()],
        "labels" => f.labels.clone(),
        "resolution" => or(f.resolution.as_ref().map(|r| r.name.clone()), "Unresolved"),
        _ => or(f.status.as_ref().map(|s| s.name.clone()), "None"),
    }
}

/// At most this many slices; the rest join "Other".
const MAX_SLICES: usize = 7;
const PIE_SIZE: f64 = 200.0;
const OUTER: f64 = 92.0;
const INNER: f64 = 58.0;

/// An SVG path for a ring slice between two angles (radians, clockwise from the top).
fn ring_slice(from: f64, to: f64) -> String {
    let c = PIE_SIZE / 2.0;
    let point = |r: f64, a: f64| (c + r * a.sin(), c - r * a.cos());
    // A full ring cannot be one arc: draw it as two halves.
    if to - from >= std::f64::consts::TAU - 1e-9 {
        let mid = from + std::f64::consts::PI;
        return format!("{} {}", ring_slice(from, mid), ring_slice(mid, to));
    }
    let large = if to - from > std::f64::consts::PI {
        1
    } else {
        0
    };
    let (ox1, oy1) = point(OUTER, from);
    let (ox2, oy2) = point(OUTER, to);
    let (ix2, iy2) = point(INNER, to);
    let (ix1, iy1) = point(INNER, from);
    format!(
        "M{ox1:.2},{oy1:.2} A{OUTER},{OUTER} 0 {large} 1 {ox2:.2},{oy2:.2} L{ix2:.2},{iy2:.2} A{INNER},{INNER} 0 {large} 0 {ix1:.2},{iy1:.2} Z"
    )
}

#[component]
fn PieChart(jql: String, stat: String) -> impl IntoView {
    let session = use_session();
    let query = StoredValue::new((jql, stat));
    let data = LocalResource::new(move || {
        session.changes.track();
        let (jql, stat) = query.get_value();
        async move {
            let issues = all_issues(
                &jql,
                &[
                    "status",
                    "assignee",
                    "priority",
                    "issuetype",
                    "project",
                    "reporter",
                    "labels",
                    "resolution",
                ],
            )
            .await?;
            let mut counts: BTreeMap<String, i64> = BTreeMap::new();
            for issue in &issues {
                for name in categories(issue, &stat) {
                    *counts.entry(name).or_default() += 1;
                }
            }
            let mut slices: Vec<(String, i64)> = counts.into_iter().collect();
            slices.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            if slices.len() > MAX_SLICES {
                let other: i64 = slices[MAX_SLICES - 1..].iter().map(|s| s.1).sum();
                slices.truncate(MAX_SLICES - 1);
                slices.push(("Other".into(), other));
            }
            Ok::<_, api::ApiError>((slices, issues.len()))
        }
    });
    let label = STAT_TYPES
        .iter()
        .find(|(k, _)| *k == query.get_value().1)
        .map_or("Status", |(_, l)| l);
    view! {
        <Transition fallback=|| view! { <p class="muted small">"Loading…"</p> }>
            {move || data.get().map(|result| match result {
                Ok((slices, _)) if slices.is_empty() => view! { <p class="muted small">"No issues."</p> }.into_any(),
                Ok((slices, issues)) => view! { <PieView slices=slices issues=issues label=label/> }.into_any(),
                Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
            })}
        </Transition>
    }
}

#[component]
fn PieView(slices: Vec<(String, i64)>, issues: usize, label: &'static str) -> impl IntoView {
    let total: i64 = slices.iter().map(|s| s.1).sum::<i64>().max(1);
    let hovered = RwSignal::new(None::<usize>);
    let mut angle = 0.0;
    let paths: Vec<(usize, String)> = slices
        .iter()
        .enumerate()
        .map(|(i, (_, count))| {
            let sweep = *count as f64 / total as f64 * std::f64::consts::TAU;
            let path = ring_slice(angle, angle + sweep);
            angle += sweep;
            (i, path)
        })
        .collect();
    let stored = StoredValue::new(slices.clone());
    let center = PIE_SIZE / 2.0;
    view! {
        <div class="chart pie">
            <svg viewBox=format!("0 0 {PIE_SIZE} {PIE_SIZE}") class="pie-svg" role="img"
                aria-label=format!("Issues by {}", label.to_lowercase())>
                {paths.into_iter().map(|(i, d)| {
                    let class = if i + 1 == stored.get_value().len() && stored.get_value()[i].0 == "Other" {
                        "slice slice-other".to_string()
                    } else {
                        format!("slice slice-{i}")
                    };
                    view! {
                        <path d=d class=class class:dimmed=move || hovered.get().is_some_and(|h| h != i)
                            on:mouseenter=move |_| hovered.set(Some(i)) on:mouseleave=move |_| hovered.set(None)/>
                    }
                }).collect_view()}
                <text x=center y=center - 4.0 class="pie-total" text-anchor="middle">
                    {move || match hovered.get() {
                        Some(i) => stored.get_value()[i].1.to_string(),
                        None => issues.to_string(),
                    }}
                </text>
                <text x=center y=center + 14.0 class="tick" text-anchor="middle">
                    {move || match hovered.get() {
                        Some(i) => stored.get_value()[i].0.clone(),
                        None => "issues".to_string(),
                    }}
                </text>
            </svg>
            <table class="pie-legend">
                <thead><tr><th>{label}</th><th class="num">"Issues"</th><th class="num">"%"</th></tr></thead>
                <tbody>
                    {slices.into_iter().enumerate().map(|(i, (name, count))| {
                        let swatch = if name == "Other" && i + 1 == stored.get_value().len() {
                            "key-swatch slice-other".to_string()
                        } else {
                            format!("key-swatch slice-{i}")
                        };
                        let percent = count as f64 * 100.0 / total as f64;
                        view! {
                            <tr class:active=move || hovered.get() == Some(i)
                                on:mouseenter=move |_| hovered.set(Some(i)) on:mouseleave=move |_| hovered.set(None)>
                                <td><span class=swatch></span>{name}</td>
                                <td class="num">{count}</td>
                                <td class="num">{format!("{percent:.0}%")}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}

const DAY: f64 = 86_400_000.0;
const LINE_WIDTH: f64 = 480.0;
const LINE_HEIGHT: f64 = 200.0;
const LEFT: f64 = 34.0;
const RIGHT: f64 = 10.0;
const TOP: f64 = 12.0;
const BOTTOM: f64 = 24.0;

/// Local midnight today, in milliseconds.
fn today() -> f64 {
    let now = js_sys::Date::new_0();
    now.set_hours(0);
    now.set_minutes(0);
    now.set_seconds(0);
    now.set_milliseconds(0);
    now.get_time()
}

fn day_label(ms: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {}",
        date.get_date(),
        MONTHS[date.get_month() as usize % 12]
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
struct DayCount {
    start: f64,
    created: i64,
    resolved: i64,
}

#[component]
fn CreatedResolvedChart(jql: String, days: i64) -> impl IntoView {
    let session = use_session();
    let query = StoredValue::new(jql);
    let data = LocalResource::new(move || {
        session.changes.track();
        let jql = query.get_value();
        async move {
            let window = format!("({jql}) AND (created >= -{days}d OR resolved >= -{days}d)");
            let issues = all_issues(&window, &["created", "resolutiondate"]).await?;
            let first = today() - (days - 1) as f64 * DAY;
            let mut counts: Vec<DayCount> = (0..days)
                .map(|i| DayCount {
                    start: first + i as f64 * DAY,
                    ..Default::default()
                })
                .collect();
            let slot = |text: &str| -> Option<usize> {
                let index = ((millis(text) - first) / DAY).floor();
                (index >= 0.0 && index < days as f64).then_some(index as usize)
            };
            for issue in &issues {
                if let Some(i) = slot(&issue.fields.created) {
                    counts[i].created += 1;
                }
                if let Some(i) = issue
                    .fields
                    .extra
                    .get("resolutiondate")
                    .and_then(Value::as_str)
                    .and_then(slot)
                {
                    counts[i].resolved += 1;
                }
            }
            Ok::<_, api::ApiError>(counts)
        }
    });
    view! {
        <Transition fallback=|| view! { <p class="muted small">"Loading…"</p> }>
            {move || data.get().map(|result| match result {
                Ok(counts) => view! { <LineChart counts=counts/> }.into_any(),
                Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
            })}
        </Transition>
    }
}

#[component]
fn LineChart(counts: Vec<DayCount>) -> impl IntoView {
    let max = counts
        .iter()
        .map(|c| c.created.max(c.resolved))
        .max()
        .unwrap_or(0) as f64;
    let y_ticks = ticks(max);
    let top = y_ticks.last().copied().unwrap_or(1.0);
    let last = counts.len().saturating_sub(1).max(1) as f64;
    let x = move |i: usize| LEFT + i as f64 / last * (LINE_WIDTH - LEFT - RIGHT);
    let y = move |v: i64| TOP + (1.0 - v as f64 / top) * (LINE_HEIGHT - TOP - BOTTOM);
    let line = |pick: fn(&DayCount) -> i64| -> String {
        counts
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let command = if i == 0 { 'M' } else { 'L' };
                format!("{command}{:.1},{:.1}", x(i), y(pick(c)))
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let created_path = line(|c| c.created);
    let resolved_path = line(|c| c.resolved);
    let totals = (
        counts.iter().map(|c| c.created).sum::<i64>(),
        counts.iter().map(|c| c.resolved).sum::<i64>(),
    );
    let first_label = counts
        .first()
        .map(|c| day_label(c.start))
        .unwrap_or_default();
    let last_label = counts
        .last()
        .map(|c| day_label(c.start))
        .unwrap_or_default();
    let hovered = RwSignal::new(None::<usize>);
    let show_table = RwSignal::new(false);
    let stored = StoredValue::new(counts);
    let on_move = move |ev: leptos::ev::MouseEvent| {
        use wasm_bindgen::JsCast;
        let Some(target) = ev.current_target() else {
            return;
        };
        let element: web_sys::Element = target.unchecked_into();
        let rect = element.get_bounding_client_rect();
        let scale = LINE_WIDTH / rect.width().max(1.0);
        let mouse = (ev.client_x() as f64 - rect.left()) * scale;
        let step = (LINE_WIDTH - LEFT - RIGHT) / last;
        let index = ((mouse - LEFT) / step).round().clamp(0.0, last) as usize;
        hovered.set(Some(index));
    };
    view! {
        <div class="chart">
            <div class="chart-legend">
                <span class="key"><span class="key-line series-1"></span>{format!("Created ({})", totals.0)}</span>
                <span class="key"><span class="key-line series-2"></span>{format!("Resolved ({})", totals.1)}</span>
                <button class="link-button small chart-toggle" on:click=move |_| show_table.update(|s| *s = !*s)>
                    {move || if show_table.get() { "Show chart" } else { "Show table" }}
                </button>
            </div>
            <Show when=move || !show_table.get() fallback=move || view! {
                <table class="gadget-table chart-table">
                    <thead><tr><th>"Day"</th><th class="num">"Created"</th><th class="num">"Resolved"</th></tr></thead>
                    <tbody>
                        {stored.get_value().into_iter().rev().map(|c| view! {
                            <tr><td>{day_label(c.start)}</td><td class="num">{c.created}</td><td class="num">{c.resolved}</td></tr>
                        }).collect_view()}
                    </tbody>
                </table>
            }>
                <svg viewBox=format!("0 0 {LINE_WIDTH} {LINE_HEIGHT}") class="chart-svg" role="img"
                    aria-label="Issues created and resolved per day"
                    on:mousemove=on_move on:mouseleave=move |_| hovered.set(None)>
                    {y_ticks.iter().map(|v| {
                        let ty = TOP + (1.0 - v / top) * (LINE_HEIGHT - TOP - BOTTOM);
                        view! {
                            <line x1=LEFT x2=LINE_WIDTH - RIGHT y1=ty y2=ty class="grid"/>
                            <text x=LEFT - 6.0 y=ty + 4.0 class="tick" text-anchor="end">{number(*v)}</text>
                        }
                    }).collect_view()}
                    <text x=LEFT y=LINE_HEIGHT - 6.0 class="tick">{first_label.clone()}</text>
                    <text x=LINE_WIDTH - RIGHT y=LINE_HEIGHT - 6.0 class="tick" text-anchor="end">{last_label.clone()}</text>
                    <path d=created_path.clone() class="series-1-line"/>
                    <path d=resolved_path.clone() class="series-2-line"/>
                    {move || hovered.get().map(|i| {
                        let c = stored.get_value()[i].clone();
                        let hx = x(i);
                        view! {
                            <line x1=hx x2=hx y1=TOP y2=LINE_HEIGHT - BOTTOM class="crosshair"/>
                            <circle cx=hx cy=y(c.created) r="4" class="series-1-dot"/>
                            <circle cx=hx cy=y(c.resolved) r="4" class="series-2-dot"/>
                        }
                    })}
                </svg>
                {move || hovered.get().map(|i| {
                    let c = stored.get_value()[i].clone();
                    view! {
                        <div class="chart-tooltip">
                            <strong>{day_label(c.start)}</strong>
                            <span class="small">{format!("Created: {}", c.created)}</span>
                            <span class="small">{format!("Resolved: {}", c.resolved)}</span>
                        </div>
                    }
                })}
            </Show>
        </div>
    }
}

/// The gadget directory: everything that can be added, each with an Add button.
#[component]
pub fn AddGadget(dashboard: String, on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let error = RwSignal::new(None::<String>);
    let available = LocalResource::new(|| async {
        api::get::<Value>(&format!("{}/dashboard/gadgets", api::CORE)).await
    });
    let dashboard = StoredValue::new(dashboard);
    let add = move |key: String| {
        spawn_local(async move {
            let path = format!("{}/dashboard/{}/gadget", api::CORE, dashboard.get_value());
            match api::post::<Value>(&path, &json!({ "moduleKey": key, "color": "blue" })).await {
                Ok(_) => {
                    session.changed();
                    on_close.run(());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Add a gadget".to_string() on_close=on_close>
            <ErrorText error=error/>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || available.get().map(|result| match result {
                    Ok(list) => view! {
                        <ul class="gadget-directory">
                            {list["gadgets"].as_array().cloned().unwrap_or_default().into_iter().map(|g| {
                                let key = g["moduleKey"].as_str().unwrap_or_default().to_string();
                                let title = g["title"].as_str().unwrap_or_default().to_string();
                                view! {
                                    <li>
                                        <span>{title}</span>
                                        <button class="button small" on:click=move |_| add(key.clone())>"Add"</button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any(),
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
        </Modal>
    }
}
