//! `/projects/:key/reports`: the burndown, sprint report, and velocity of a scrum board.

use leptos::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use super::projects::use_project;
use crate::api;
use crate::model::{Page, Sprint, short_time};

/// Chart size in SVG units. The SVG scales to the width of its container.
const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 260.0;
const LEFT: f64 = 44.0;
const RIGHT: f64 = 16.0;
const TOP: f64 = 16.0;
const BOTTOM: f64 = 28.0;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Point {
    time: String,
    remaining: f64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Burndown {
    points: Vec<Point>,
    guideline: Vec<Value>,
}

/// Milliseconds since the epoch for an API date-time.
fn millis(text: &str) -> f64 {
    js_sys::Date::parse(&text.replacen("+0000", "Z", 1))
}

/// Clean tick values from zero to at least `max`.
fn ticks(max: f64) -> Vec<f64> {
    let max = max.max(1.0);
    let raw = max / 4.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|s| *s >= raw)
        .unwrap_or(magnitude * 10.0);
    (0..)
        .map(|i| i as f64 * step)
        .take_while(|v| *v <= max + step * 0.999)
        .collect()
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[component]
pub fn Reports() -> impl IntoView {
    let project = use_project();
    let sprints = LocalResource::new(move || async move {
        let Some(board) = project.board.await else {
            return Vec::new();
        };
        api::get::<Page<Sprint>>(&format!(
            "{}/board/{}/sprint?state=active,closed",
            api::AGILE,
            board.id
        ))
        .await
        .map(|p| p.values)
        .unwrap_or_default()
    });
    let chosen = RwSignal::new(None::<i64>);
    // Show the most recent sprint first.
    Effect::new(move |_| {
        if chosen.get_untracked().is_none()
            && let Some(last) = sprints.get().and_then(|list| list.last().cloned())
        {
            chosen.set(Some(last.id));
        }
    });
    view! {
        <div class="page reports-page">
            <header class="page-header">
                <h1>"Reports"</h1>
                <select class="select sprint-picker" on:change:target=move |ev| chosen.set(ev.target().value().parse().ok())>
                    {move || sprints.get().unwrap_or_default().into_iter().rev().map(|s| {
                        let selected = Some(s.id) == chosen.get_untracked();
                        view! { <option value=s.id.to_string() selected=selected>{s.name}</option> }
                    }).collect_view()}
                </select>
            </header>
            {move || match chosen.get() {
                Some(id) => view! { <SprintReports sprint=id/> }.into_any(),
                None => view! { <p class="empty">"Start a sprint to see its reports."</p> }.into_any(),
            }}
            <Velocity/>
        </div>
    }
}

#[component]
fn SprintReports(sprint: i64) -> impl IntoView {
    let burndown = LocalResource::new(move || async move {
        api::get::<Burndown>(&format!(
            "/rest/tessera/1.0/reports/burndown?sprintId={sprint}"
        ))
        .await
    });
    let project = use_project();
    let report = LocalResource::new(move || async move {
        let board = project.board.await.map(|b| b.id).unwrap_or_default();
        api::get::<Value>(&format!(
            "/rest/greenhopper/1.0/rapid/charts/sprintreport?rapidViewId={board}&sprintId={sprint}"
        ))
        .await
    });
    view! {
        <section class="settings-card">
            <h2>"Burndown"</h2>
            <p class="muted small">"Story points left in the sprint over time."</p>
            {move || burndown.get().map(|result| match result {
                Ok(data) if data.points.len() > 1 => view! { <BurndownChart data=data/> }.into_any(),
                Ok(_) => view! { <p class="muted">"No changes yet."</p> }.into_any(),
                Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
            })}
        </section>
        <section class="settings-card">
            <h2>"Sprint report"</h2>
            {move || report.get().map(|result| match result {
                Ok(data) => view! { <ReportLists data=data/> }.into_any(),
                Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
            })}
        </section>
    }
}

#[component]
fn BurndownChart(data: Burndown) -> impl IntoView {
    let times: Vec<f64> = data.points.iter().map(|p| millis(&p.time)).collect();
    let guide_end = data
        .guideline
        .get(1)
        .and_then(|g| g["time"].as_str())
        .map(millis);
    let start = times.first().copied().unwrap_or_default();
    let end = times
        .iter()
        .copied()
        .chain(guide_end)
        .fold(start + 1.0, f64::max);
    let max = data.points.iter().map(|p| p.remaining).fold(0.0, f64::max);
    let y_ticks = ticks(max);
    let top_value = y_ticks.last().copied().unwrap_or(1.0);
    let x = move |t: f64| LEFT + (t - start) / (end - start) * (WIDTH - LEFT - RIGHT);
    let y = move |v: f64| TOP + (1.0 - v / top_value) * (HEIGHT - TOP - BOTTOM);
    // A step line: the value holds until the next change.
    let mut path = String::new();
    for (i, point) in data.points.iter().enumerate() {
        let (px, py) = (x(times[i]), y(point.remaining));
        if i == 0 {
            path.push_str(&format!("M{px:.1},{py:.1}"));
        } else {
            path.push_str(&format!(" H{px:.1} V{py:.1}"));
        }
    }
    let guide = format!(
        "M{:.1},{:.1} L{:.1},{:.1}",
        x(start),
        y(data.points.first().map_or(0.0, |p| p.remaining)),
        x(guide_end.unwrap_or(end)),
        y(0.0)
    );
    let last = data.points.last().cloned().unwrap_or_default();
    let last_x = x(times.last().copied().unwrap_or(start));
    let hovered = RwSignal::new(None::<usize>);
    let points = StoredValue::new((data.points.clone(), times.clone()));
    let on_move = move |ev: leptos::ev::MouseEvent| {
        let Some(target) = ev.current_target() else {
            return;
        };
        use wasm_bindgen::JsCast;
        let element: web_sys::Element = target.unchecked_into();
        let rect = element.get_bounding_client_rect();
        let scale = WIDTH / rect.width().max(1.0);
        let mouse = (ev.client_x() as f64 - rect.left()) * scale;
        let (_, times) = points.get_value();
        // The step in effect under the pointer.
        let index = times.iter().rposition(|t| x(*t) <= mouse).unwrap_or(0);
        hovered.set(Some(index));
    };
    view! {
        <div class="chart">
            <div class="chart-legend">
                <span class="key"><span class="key-line series-1"></span>"Remaining"</span>
                <span class="key"><span class="key-line guide"></span>"Guideline"</span>
            </div>
            <svg viewBox=format!("0 0 {WIDTH} {HEIGHT}") class="chart-svg" role="img"
                aria-label="Burndown: story points remaining over the sprint"
                on:mousemove=on_move on:mouseleave=move |_| hovered.set(None)>
                {y_ticks.iter().map(|v| {
                    let ty = y(*v);
                    view! {
                        <line x1=LEFT x2=WIDTH - RIGHT y1=ty y2=ty class="grid"/>
                        <text x=LEFT - 8.0 y=ty + 4.0 class="tick" text-anchor="end">{number(*v)}</text>
                    }
                }).collect_view()}
                <text x=LEFT y=HEIGHT - 8.0 class="tick">{short_time(&data.points[0].time)}</text>
                <path d=guide class="guide-line"/>
                <path d=path class="series-1-line"/>
                <circle cx=last_x cy=y(last.remaining) r="4" class="series-1-dot"/>
                <text x=last_x + 8.0 y=y(last.remaining) - 6.0 class="value">{number(last.remaining)}</text>
                {move || hovered.get().map(|i| {
                    let (points, times) = points.get_value();
                    let hx = x(times[i]);
                    view! { <line x1=hx x2=hx y1=TOP y2=HEIGHT - BOTTOM class="crosshair"/>
                            <circle cx=hx cy=y(points[i].remaining) r="4" class="series-1-dot"/> }
                })}
            </svg>
            {move || hovered.get().map(|i| {
                let (points, _) = points.get_value();
                view! {
                    <div class="chart-tooltip">
                        <strong>{number(points[i].remaining)}" points left"</strong>
                        <span class="muted small">{short_time(&points[i].time)}</span>
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn ReportLists(data: Value) -> impl IntoView {
    let contents = data["contents"].clone();
    let added = contents["issueKeysAddedDuringSprint"].clone();
    let section = move |title: &'static str, key: &str, sum: &str| {
        let issues = contents[key].as_array().cloned().unwrap_or_default();
        let total = contents[sum]["text"].as_str().unwrap_or("0").to_string();
        let added = added.clone();
        view! {
            <h3 class="report-heading">{format!("{title} ({})", issues.len())}<span class="muted small">{format!("{total} points")}</span></h3>
            {if issues.is_empty() {
                view! { <p class="muted small">"None."</p> }.into_any()
            } else {
                view! {
                    <table class="issue-table">
                        <thead><tr><th>"Key"</th><th>"Summary"</th><th>"Status"</th><th>"Points"</th></tr></thead>
                        <tbody>
                            {issues.into_iter().map(|issue| {
                                let key = issue["key"].as_str().unwrap_or_default().to_string();
                                let late = added[&key] == true;
                                let points = issue["currentEstimateStatistic"]["statFieldValue"]["text"].as_str().unwrap_or("").replace("null", "–");
                                view! {
                                    <tr>
                                        <td><a href=format!("/browse/{key}")>{key.clone()}</a>{late.then(|| view! { <span class="muted small">" *"</span> })}</td>
                                        <td>{issue["summary"].as_str().unwrap_or_default().to_string()}</td>
                                        <td class="muted">{issue["statusName"].as_str().unwrap_or_default().to_string()}</td>
                                        <td>{points}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_any()
            }}
        }
    };
    view! {
        {section("Completed", "completedIssues", "completedIssuesEstimateSum")}
        {section("Not completed", "issuesNotCompletedInCurrentSprint", "issuesNotCompletedEstimateSum")}
        {section("Removed from the sprint", "puntedIssues", "puntedIssuesEstimateSum")}
        <p class="muted small">"* Added after the sprint started."</p>
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct VelocityRow {
    name: String,
    committed: f64,
    completed: f64,
}

#[component]
fn Velocity() -> impl IntoView {
    let project = use_project();
    let rows = LocalResource::new(move || async move {
        let Some(board) = project.board.await else {
            return Vec::new();
        };
        let data: Value = api::get(&format!(
            "/rest/greenhopper/1.0/rapid/charts/velocity?rapidViewId={}",
            board.id
        ))
        .await
        .unwrap_or_default();
        data["sprints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|s| {
                let entry = &data["velocityStatEntries"][s["id"].to_string()];
                VelocityRow {
                    name: s["name"].as_str().unwrap_or_default().to_string(),
                    committed: entry["estimated"]["value"].as_f64().unwrap_or(0.0),
                    completed: entry["completed"]["value"].as_f64().unwrap_or(0.0),
                }
            })
            .collect::<Vec<_>>()
    });
    view! {
        <section class="settings-card">
            <h2>"Velocity"</h2>
            <p class="muted small">"Story points committed and completed in recent sprints."</p>
            {move || rows.get().map(|rows| if rows.is_empty() {
                view! { <p class="muted">"Complete a sprint to see velocity."</p> }.into_any()
            } else {
                view! { <VelocityChart rows=rows/> }.into_any()
            })}
        </section>
    }
}

#[component]
fn VelocityChart(rows: Vec<VelocityRow>) -> impl IntoView {
    let max = rows
        .iter()
        .map(|r| r.committed.max(r.completed))
        .fold(0.0, f64::max);
    let y_ticks = ticks(max);
    let top_value = y_ticks.last().copied().unwrap_or(1.0);
    let y = move |v: f64| TOP + (1.0 - v / top_value) * (HEIGHT - TOP - BOTTOM);
    let band = (WIDTH - LEFT - RIGHT) / rows.len() as f64;
    let bar = 24f64.min(band / 3.0);
    let hovered = RwSignal::new(None::<(usize, bool)>);
    let stored = StoredValue::new(rows.clone());
    // A column with a rounded top and a square base.
    let column = |x0: f64, value: f64| {
        let top = y(value);
        let base = y(0.0);
        let r = 4f64.min((base - top) / 2.0).max(0.0);
        format!(
            "M{x0:.1},{base:.1} V{:.1} Q{x0:.1},{top:.1} {:.1},{top:.1} H{:.1} Q{:.1},{top:.1} {:.1},{:.1} V{base:.1} Z",
            top + r,
            x0 + r,
            x0 + bar - r,
            x0 + bar,
            x0 + bar,
            top + r
        )
    };
    view! {
        <div class="chart">
            <div class="chart-legend">
                <span class="key"><span class="key-swatch series-2"></span>"Committed"</span>
                <span class="key"><span class="key-swatch series-1"></span>"Completed"</span>
            </div>
            <svg viewBox=format!("0 0 {WIDTH} {HEIGHT}") class="chart-svg" role="img" aria-label="Velocity by sprint">
                {y_ticks.iter().map(|v| {
                    let ty = y(*v);
                    view! {
                        <line x1=LEFT x2=WIDTH - RIGHT y1=ty y2=ty class="grid"/>
                        <text x=LEFT - 8.0 y=ty + 4.0 class="tick" text-anchor="end">{number(*v)}</text>
                    }
                }).collect_view()}
                {rows.iter().enumerate().map(|(i, row)| {
                    let center = LEFT + band * (i as f64 + 0.5);
                    // Two columns side by side, 2px apart.
                    let first = center - bar - 1.0;
                    let second = center + 1.0;
                    view! {
                        <path d=column(first, row.committed) class="series-2-fill"
                            on:mouseenter=move |_| hovered.set(Some((i, false))) on:mouseleave=move |_| hovered.set(None)/>
                        <path d=column(second, row.completed) class="series-1-fill"
                            on:mouseenter=move |_| hovered.set(Some((i, true))) on:mouseleave=move |_| hovered.set(None)/>
                        <text x=second + bar / 2.0 y=y(row.completed) - 6.0 class="value" text-anchor="middle">{number(row.completed)}</text>
                        <text x=center y=HEIGHT - 8.0 class="tick" text-anchor="middle">{row.name.clone()}</text>
                    }
                }).collect_view()}
            </svg>
            {move || hovered.get().map(|(i, completed)| {
                let row = stored.get_value()[i].clone();
                let (label, value) = if completed { ("Completed", row.completed) } else { ("Committed", row.committed) };
                view! {
                    <div class="chart-tooltip">
                        <strong>{format!("{label}: {} points", number(value))}</strong>
                        <span class="muted small">{row.name}</span>
                    </div>
                }
            })}
            <table class="issue-table chart-table">
                <thead><tr><th>"Sprint"</th><th>"Committed"</th><th>"Completed"</th></tr></thead>
                <tbody>
                    {rows.into_iter().map(|r| view! {
                        <tr><td>{r.name}</td><td>{number(r.committed)}</td><td>{number(r.completed)}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}
