//! The board (columns of cards) and the backlog (sprints and unplanned work).
//!
//! Both use drag and drop. On the board, dropping a card in another column runs the workflow
//! transition to that column; dropping it on a card ranks it before that card. In the backlog,
//! dropping an issue on a sprint or the backlog moves it there.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, Redirect};
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};

use super::{AgileFields, agile_fields};
use crate::api;
use crate::app::use_session;
use crate::components::{Avatar, ErrorText, Lozenge, Modal, TypeIcon};
use crate::model::{Board, BoardConfig, Issue, Page, SearchPage, Sprint, Transition};
use crate::pages::projects::use_project;

/// `/projects/:key`: the board if the project has one, else the issue list.
#[component]
pub fn ProjectHome() -> impl IntoView {
    let project = use_project();
    view! {
        <Suspense fallback=|| ()>
            {move || project.board.get().map(|board| match board {
                Some(board) if board.board_type == "scrum" => view! { <Redirect path="backlog"/> }.into_any(),
                Some(_) => view! { <Redirect path="board"/> }.into_any(),
                None => view! { <Redirect path="issues"/> }.into_any(),
            })}
        </Suspense>
    }
}

/// Story points of an issue, if it has an estimate.
fn points(issue: &Issue, fields: &AgileFields) -> Option<String> {
    let id = fields.story_points.as_ref()?;
    let value = issue.fields.extra.get(id)?.as_f64()?;
    Some(if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    })
}

/// Loads the issues of an Agile issue list endpoint.
async fn agile_issues(path: &str) -> Vec<Issue> {
    let separator = if path.contains('?') { '&' } else { '?' };
    api::get::<SearchPage>(&format!("{}{path}{separator}maxResults=1000", api::AGILE))
        .await
        .map(|page| page.issues)
        .unwrap_or_default()
}

/// A card on the board.
#[component]
fn Card(
    issue: Issue,
    points: Option<String>,
    dragging: RwSignal<Option<String>>,
    on_drop_before: Callback<String>,
) -> impl IntoView {
    let navigate = use_navigate();
    let key = issue.key.clone();
    let drag_key = key.clone();
    let target_key = key.clone();
    let fields = issue.fields;
    view! {
        <article class="card" draggable="true"
            on:dragstart=move |_| dragging.set(Some(drag_key.clone()))
            on:dragend=move |_| dragging.set(None)
            on:dragover=|ev| ev.prevent_default()
            on:drop=move |ev| {
                ev.prevent_default();
                ev.stop_propagation();
                on_drop_before.run(target_key.clone());
            }
            on:click=move |_| navigate(&format!("/browse/{key}"), Default::default())>
            <p class="card-summary">{fields.summary}</p>
            <footer class="card-footer">
                <TypeIcon item=fields.issuetype/>
                <span class="issue-key">{issue.key}</span>
                <span class="spacer"></span>
                {points.map(|p| view! { <span class="points">{p}</span> })}
                <TypeIcon item=fields.priority/>
                <Avatar user=fields.assignee size=20/>
            </footer>
        </article>
    }
}

/// `/projects/:key/board`: columns of cards.
#[component]
pub fn BoardPage() -> impl IntoView {
    let project = use_project();
    view! {
        <Transition fallback=|| view! { <p class="page muted">"Loading board…"</p> }>
            {move || project.board.get().map(|board| match board {
                Some(board) => view! { <BoardView board=board/> }.into_any(),
                None => view! { <div class="page"><p class="empty">"This project has no board."</p></div> }.into_any(),
            })}
        </Transition>
    }
}

#[component]
fn BoardView(board: Board) -> impl IntoView {
    let session = use_session();
    let scrum = board.board_type == "scrum";
    let board_id = board.id;
    let config = LocalResource::new(move || async move {
        api::get::<BoardConfig>(&format!("{}/board/{board_id}/configuration", api::AGILE))
            .await
            .unwrap_or_default()
    });
    let active_sprint = LocalResource::new(move || {
        session.changes.track();
        async move {
            if !scrum {
                return None;
            }
            api::get::<Page<Sprint>>(&format!(
                "{}/board/{board_id}/sprint?state=active",
                api::AGILE
            ))
            .await
            .ok()
            .and_then(|page| page.values.into_iter().next())
        }
    });
    let issues = LocalResource::new(move || {
        session.changes.track();
        async move {
            let jql = if scrum {
                "&jql=sprint%20in%20openSprints()"
            } else {
                ""
            };
            agile_issues(&format!("/board/{board_id}/issue?fields=*all{jql}")).await
        }
    });
    let fields = agile_fields();
    let dragging = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);

    // Moves the dragged issue into a column by running a transition to one of its statuses.
    let drop_in_column = move |status_ids: Vec<String>| {
        let Some(key) = dragging.get_untracked() else {
            return;
        };
        let current = issues
            .get_untracked()
            .unwrap_or_default()
            .into_iter()
            .find(|i| i.key == key)
            .and_then(|i| i.fields.status.map(|s| s.id));
        if current.is_some_and(|id| status_ids.contains(&id)) {
            return;
        }
        spawn_local(async move {
            let list: Value = api::get(&format!("{}/issue/{key}/transitions", api::CORE))
                .await
                .unwrap_or_default();
            let transitions: Vec<Transition> =
                serde_json::from_value(list["transitions"].clone()).unwrap_or_default();
            match transitions.iter().find(|t| status_ids.contains(&t.to.id)) {
                Some(t) => {
                    let body = json!({ "transition": { "id": t.id } });
                    match api::request(
                        gloo_net::http::Method::POST,
                        &format!("{}/issue/{key}/transitions", api::CORE),
                        Some(&body),
                    )
                    .await
                    {
                        Ok(_) => session.changed(),
                        Err(e) => error.set(Some(e.message)),
                    }
                }
                None => error.set(Some(format!(
                    "{key} cannot move to that column from its current status."
                ))),
            }
        });
    };
    // Ranks the dragged issue before another card.
    let drop_before = Callback::new(move |target: String| {
        let Some(key) = dragging.get_untracked() else {
            return;
        };
        if key == target {
            return;
        }
        spawn_local(async move {
            let body = json!({ "issues": [key], "rankBeforeIssue": target });
            match api::request(
                gloo_net::http::Method::PUT,
                &format!("{}/issue/rank", api::AGILE),
                Some(&body),
            )
            .await
            {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });
    let completing = RwSignal::new(false);

    view! {
        <div class="page board-page">
            <header class="page-header">
                <div>
                    <h1>{move || active_sprint.get().flatten().map(|s| s.name).unwrap_or_else(|| board.name.clone())}</h1>
                    {move || active_sprint.get().flatten().filter(|s| !s.goal.is_empty()).map(|s| view! { <p class="muted">{s.goal}</p> })}
                </div>
                {move || active_sprint.get().flatten().map(|_| view! {
                    <button class="button" on:click=move |_| completing.set(true)>"Complete sprint"</button>
                })}
            </header>
            <ErrorText error=error/>
            {move || (scrum && active_sprint.get().is_some_and(|s| s.is_none())).then(|| view! {
                <p class="empty">"No sprint is active. Start a sprint from the " <A href="../backlog">"backlog"</A> "."</p>
            })}
            <div class="columns">
                {move || {
                    let all = issues.get().unwrap_or_default();
                    let agile = fields.get().unwrap_or_default();
                    config.get().unwrap_or_default().column_config.columns.into_iter().map(|column| {
                        let status_ids: Vec<String> = column.statuses.iter().map(|s| s.id.clone()).collect();
                        let cards: Vec<Issue> = all
                            .iter()
                            .filter(|i| i.fields.status.as_ref().is_some_and(|s| status_ids.contains(&s.id)))
                            .cloned()
                            .collect();
                        let count = cards.len();
                        let ids = status_ids.clone();
                        let agile = agile.clone();
                        view! {
                            <section class="column"
                                on:dragover=|ev| ev.prevent_default()
                                on:drop=move |ev| { ev.prevent_default(); drop_in_column(ids.clone()); }>
                                <h2 class="column-title">{column.name}<span class="count">{count}</span></h2>
                                {cards.into_iter().map(|issue| {
                                    let points = points(&issue, &agile);
                                    view! { <Card issue=issue points=points dragging=dragging on_drop_before=drop_before/> }
                                }).collect_view()}
                            </section>
                        }
                    }).collect_view()
                }}
            </div>
            <Show when=move || completing.get()>
                {move || active_sprint.get().flatten().map(|sprint| view! {
                    <CompleteSprint sprint=sprint on_close=Callback::new(move |()| completing.set(false))/>
                })}
            </Show>
        </div>
    }
}

/// Confirms completing a sprint. Unfinished issues go back to the backlog.
#[component]
fn CompleteSprint(sprint: Sprint, on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let error = RwSignal::new(None::<String>);
    let id = sprint.id;
    let complete = move |_| {
        spawn_local(async move {
            match api::post::<Value>(
                &format!("{}/sprint/{id}", api::AGILE),
                &json!({ "state": "closed" }),
            )
            .await
            {
                Ok(_) => {
                    session.changed();
                    on_close.run(());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title=format!("Complete {}", sprint.name) on_close=on_close>
            <p>"Issues that are not done will move to the backlog."</p>
            <ErrorText error=error/>
            <div class="form-actions">
                <button class="button primary" on:click=complete>"Complete sprint"</button>
            </div>
        </Modal>
    }
}

/// `/projects/:key/backlog`: sprints and the backlog.
#[component]
pub fn BacklogPage() -> impl IntoView {
    let project = use_project();
    view! {
        <Transition fallback=|| view! { <p class="page muted">"Loading backlog…"</p> }>
            {move || project.board.get().map(|board| match board {
                Some(board) if board.board_type == "scrum" => view! { <BacklogView board=board/> }.into_any(),
                _ => view! { <div class="page"><p class="empty">"This project has no scrum board."</p></div> }.into_any(),
            })}
        </Transition>
    }
}

/// Where a dragged issue can go in the backlog view.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Sprint(i64),
    Backlog,
}

#[component]
fn BacklogView(board: Board) -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let board_id = board.id;
    let sprints = LocalResource::new(move || {
        session.changes.track();
        async move {
            api::get::<Page<Sprint>>(&format!(
                "{}/board/{board_id}/sprint?state=active,future",
                api::AGILE
            ))
            .await
            .map(|page| page.values)
            .unwrap_or_default()
        }
    });
    let backlog = LocalResource::new(move || {
        session.changes.track();
        async move { agile_issues(&format!("/board/{board_id}/backlog?fields=*all")).await }
    });
    let fields = agile_fields();
    let dragging = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let starting = RwSignal::new(None::<Sprint>);

    let move_to = Callback::new(move |target: Target| {
        let Some(key) = dragging.get_untracked() else {
            return;
        };
        let (path, body) = match target {
            Target::Sprint(id) => (
                format!("{}/sprint/{id}/issue", api::AGILE),
                json!({ "issues": [key] }),
            ),
            Target::Backlog => (
                format!("{}/backlog/issue", api::AGILE),
                json!({ "issues": [key] }),
            ),
        };
        spawn_local(async move {
            match api::request(gloo_net::http::Method::POST, &path, Some(&body)).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });
    let create_sprint = move |_| {
        let count = sprints.get_untracked().map(|s| s.len()).unwrap_or(0);
        let name = format!("{} Sprint {}", project.key.get_untracked(), count + 1);
        spawn_local(async move {
            let body = json!({ "name": name, "originBoardId": board_id });
            match api::post::<Value>(&format!("{}/sprint", api::AGILE), &body).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="page backlog-page">
            <header class="page-header">
                <h1>"Backlog"</h1>
                <button class="button" on:click=create_sprint>"Create sprint"</button>
            </header>
            <ErrorText error=error/>
            {move || {
                let agile = fields.get().unwrap_or_default();
                let any_active = sprints.get().unwrap_or_default().iter().any(|s| s.state == "active");
                sprints.get().unwrap_or_default().into_iter().map(|sprint| {
                    let agile = agile.clone();
                    view! { <SprintSection sprint=sprint any_active=any_active agile=agile dragging=dragging move_to=move_to starting=starting/> }
                }).collect_view()
            }}
            <section class="backlog-list"
                on:dragover=|ev| ev.prevent_default()
                on:drop=move |ev| { ev.prevent_default(); move_to.run(Target::Backlog); }>
                <h2 class="list-title">"Backlog" <span class="count">{move || backlog.get().map(|b| b.len()).unwrap_or(0)}</span></h2>
                {move || {
                    let agile = fields.get().unwrap_or_default();
                    backlog.get().unwrap_or_default().into_iter().map(|issue| {
                        let points = points(&issue, &agile);
                        view! { <BacklogRow issue=issue points=points dragging=dragging/> }
                    }).collect_view()
                }}
                <QuickCreate/>
            </section>
            <Show when=move || starting.get().is_some()>
                {move || starting.get().map(|sprint| view! {
                    <StartSprint sprint=sprint on_close=Callback::new(move |()| starting.set(None))/>
                })}
            </Show>
        </div>
    }
}

#[component]
fn SprintSection(
    sprint: Sprint,
    any_active: bool,
    agile: AgileFields,
    dragging: RwSignal<Option<String>>,
    move_to: Callback<Target>,
    starting: RwSignal<Option<Sprint>>,
) -> impl IntoView {
    let session = use_session();
    let id = sprint.id;
    let issues = LocalResource::new(move || {
        session.changes.track();
        async move { agile_issues(&format!("/sprint/{id}/issue?fields=*all")).await }
    });
    let active = sprint.state == "active";
    let dates = match (&sprint.start_date, &sprint.end_date) {
        (Some(start), Some(end)) => format!(
            "{} – {}",
            &start[..10.min(start.len())],
            &end[..10.min(end.len())]
        ),
        _ => String::new(),
    };
    let to_start = sprint.clone();
    view! {
        <section class="sprint"
            on:dragover=|ev| ev.prevent_default()
            on:drop=move |ev| { ev.prevent_default(); move_to.run(Target::Sprint(id)); }>
            <header class="sprint-header">
                <h2 class="list-title">{sprint.name.clone()}
                    <span class="count">{move || issues.get().map(|i| i.len()).unwrap_or(0)}</span>
                </h2>
                <span class="muted small">{dates}</span>
                <span class="spacer"></span>
                {if active {
                    view! { <A href="../board" attr:class="button">"View board"</A> }.into_any()
                } else {
                    view! {
                        <button class="button" disabled=any_active on:click=move |_| starting.set(Some(to_start.clone()))>
                            "Start sprint"
                        </button>
                    }.into_any()
                }}
            </header>
            {(!sprint.goal.is_empty()).then(|| view! { <p class="muted small">{sprint.goal.clone()}</p> })}
            {move || {
                let list = issues.get().unwrap_or_default();
                if list.is_empty() {
                    view! { <p class="drop-hint">"Drag issues here to plan this sprint."</p> }.into_any()
                } else {
                    list.into_iter().map(|issue| {
                        let points = points(&issue, &agile);
                        view! { <BacklogRow issue=issue points=points dragging=dragging/> }
                    }).collect_view().into_any()
                }
            }}
        </section>
    }
}

#[component]
fn BacklogRow(
    issue: Issue,
    points: Option<String>,
    dragging: RwSignal<Option<String>>,
) -> impl IntoView {
    let key = issue.key.clone();
    let href = format!("/browse/{}", issue.key);
    let fields = issue.fields;
    view! {
        <div class="backlog-row" draggable="true"
            on:dragstart=move |_| dragging.set(Some(key.clone()))
            on:dragend=move |_| dragging.set(None)>
            <TypeIcon item=fields.issuetype/>
            <A href=href.clone() attr:class="issue-key">{issue.key}</A>
            <A href=href attr:class="summary-link grow">{fields.summary}</A>
            <Lozenge status=fields.status/>
            {points.map(|p| view! { <span class="points">{p}</span> })}
            <TypeIcon item=fields.priority/>
            <Avatar user=fields.assignee size=20/>
        </div>
    }
}

/// "What needs to be done?": creates a story in the backlog.
#[component]
fn QuickCreate() -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let summary = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = summary.get().trim().to_string();
        if text.is_empty() {
            return;
        }
        let body = json!({ "fields": {
            "project": { "key": project.key.get() },
            "issuetype": { "name": "Story" },
            "summary": text,
        }});
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/issue", api::CORE), &body).await {
                Ok(_) => {
                    summary.set(String::new());
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <form class="quick-create" on:submit=submit>
            <input class="input" placeholder="+ What needs to be done?"
                prop:value=move || summary.get()
                on:input:target=move |ev| summary.set(ev.target().value())/>
            <ErrorText error=error/>
        </form>
    }
}

/// The current time plus `days`, in the ISO format the API accepts.
fn iso_in_days(days: f64) -> String {
    let ms = js_sys::Date::now() + days * 86_400_000.0;
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

#[component]
fn StartSprint(sprint: Sprint, on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(sprint.name.clone());
    let weeks = RwSignal::new("2".to_string());
    let goal = RwSignal::new(sprint.goal.clone());
    let error = RwSignal::new(None::<String>);
    let id = sprint.id;
    let start = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let days = weeks.get().parse::<f64>().unwrap_or(2.0) * 7.0;
        let body = json!({
            "name": name.get(),
            "goal": goal.get(),
            "state": "active",
            "startDate": iso_in_days(0.0),
            "endDate": iso_in_days(days),
        });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/sprint/{id}", api::AGILE), &body).await {
                Ok(_) => {
                    session.changed();
                    on_close.run(());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Start sprint".into() on_close=on_close>
            <form class="form" on:submit=start>
                <label>"Sprint name"
                    <input class="input" required prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())/>
                </label>
                <label>"Duration"
                    <select class="select" on:change:target=move |ev| weeks.set(ev.target().value())>
                        <option value="1">"1 week"</option>
                        <option value="2" selected>"2 weeks"</option>
                        <option value="3">"3 weeks"</option>
                        <option value="4">"4 weeks"</option>
                    </select>
                </label>
                <label>"Sprint goal"
                    <textarea class="input" rows="3" prop:value=move || goal.get()
                        on:input:target=move |ev| goal.set(ev.target().value())></textarea>
                </label>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" type="submit">"Start"</button>
                </div>
            </form>
        </Modal>
    }
}
