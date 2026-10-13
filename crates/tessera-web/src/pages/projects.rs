//! The project directory, project creation, and the layout of a project's pages.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, Outlet};
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{Avatar, ErrorText, Modal};
use crate::model::{Board, Page, Project};

/// The project a page belongs to, provided by [`ProjectLayout`].
#[derive(Clone, Copy)]
pub struct ProjectContext {
    pub key: Signal<String>,
    /// The project's first board, if it has one.
    pub board: LocalResource<Option<Board>>,
}

pub fn use_project() -> ProjectContext {
    expect_context::<ProjectContext>()
}

#[component]
pub fn Projects() -> impl IntoView {
    let session = use_session();
    let projects = LocalResource::new(move || {
        session.changes.track();
        async { api::get::<Vec<Project>>(&format!("{}/project?expand=lead", api::CORE)).await }
    });
    let creating = RwSignal::new(false);
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Projects"</h1>
                <button class="button primary" on:click=move |_| creating.set(true)>"Create project"</button>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading projects…"</p> }>
                {move || projects.get().map(|result| match result {
                    Ok(list) if list.is_empty() => view! {
                        <p class="empty">"No projects yet. Create one to start tracking work."</p>
                    }.into_any(),
                    Ok(list) => view! {
                        <table class="issue-table">
                            <thead><tr><th>"Name"</th><th>"Key"</th><th>"Type"</th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|project| {
                                    let href = format!("/projects/{}", project.key);
                                    view! {
                                        <tr>
                                            <td>
                                                <span class="person">
                                                    <img class="avatar square" src=project.avatar() alt=""/>
                                                    <A href=href attr:class="summary-link">{project.name}</A>
                                                </span>
                                            </td>
                                            <td>{project.key}</td>
                                            <td class="muted">{project.project_type_key}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any(),
                    Err(error) => view! { <p class="error-text">{error.message}</p> }.into_any(),
                })}
            </Transition>
            <Show when=move || creating.get()>
                <CreateProject on_close=Callback::new(move |()| creating.set(false))/>
            </Show>
        </div>
    }
}

/// A project key suggested from a name: the first letters of its words, or its first letters.
fn suggest_key(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let key: String = if words.len() > 1 {
        words.iter().filter_map(|w| w.chars().next()).collect()
    } else {
        name.chars().take(4).collect()
    };
    let key: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    key.trim_start_matches(|c: char| c.is_ascii_digit())
        .chars()
        .take(10)
        .collect()
}

#[component]
fn CreateProject(on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let name = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    let key_edited = RwSignal::new(false);
    let template = RwSignal::new("scrum".to_string());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let (project_type, template_key) = match template.get().as_str() {
            "kanban" => (
                "software",
                "com.pyxis.greenhopper.jira:gh-simplified-kanban-classic",
            ),
            "business" => (
                "business",
                "com.atlassian.jira-core-project-templates:jira-core-simplified-task-tracking",
            ),
            _ => (
                "software",
                "com.pyxis.greenhopper.jira:gh-simplified-scrum-classic",
            ),
        };
        let body = json!({
            "name": name.get(),
            "key": key.get(),
            "projectTypeKey": project_type,
            "projectTemplateKey": template_key,
            "leadAccountId": session.me.get_value().account_id,
        });
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/project", api::CORE), &body).await {
                Ok(created) => {
                    session.changed();
                    on_close.run(());
                    navigate(
                        &format!("/projects/{}", created["key"].as_str().unwrap_or_default()),
                        Default::default(),
                    );
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Create project".into() on_close=on_close>
            <form class="form" on:submit=submit>
                <label>"Name"
                    <input class="input" required
                        prop:value=move || name.get()
                        on:input:target=move |ev| {
                            let value = ev.target().value();
                            if !key_edited.get() {
                                key.set(suggest_key(&value));
                            }
                            name.set(value);
                        }/>
                </label>
                <label>"Key"
                    <input class="input" required maxlength="10"
                        prop:value=move || key.get()
                        on:input:target=move |ev| {
                            key_edited.set(true);
                            key.set(ev.target().value().to_uppercase());
                        }/>
                </label>
                <fieldset class="choices">
                    <legend>"Template"</legend>
                    {[("scrum", "Scrum", "Plan work in sprints with a backlog."),
                      ("kanban", "Kanban", "Visualize a continuous flow of work."),
                      ("business", "Task tracking", "Track tasks without a board.")]
                        .into_iter()
                        .map(|(value, label, hint)| view! {
                            <label class="choice">
                                <input type="radio" name="template" value=value
                                    prop:checked=move || template.get() == value
                                    on:change=move |_| template.set(value.to_string())/>
                                <span><strong>{label}</strong><br/><span class="muted">{hint}</span></span>
                            </label>
                        })
                        .collect_view()}
                </fieldset>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" type="submit">"Create"</button>
                </div>
            </form>
        </Modal>
    }
}

/// The frame of a project's pages: a sidebar with its views, and the current view.
#[component]
pub fn ProjectLayout() -> impl IntoView {
    let params = use_params_map();
    let key = Signal::derive(move || {
        params
            .with(|p| p.get("key"))
            .unwrap_or_default()
            .to_uppercase()
    });
    let project = LocalResource::new(move || {
        let key = key.get();
        async move {
            api::get::<Project>(&format!("{}/project/{key}", api::CORE))
                .await
                .ok()
        }
    });
    let board = LocalResource::new(move || {
        let key = key.get();
        async move {
            api::get::<Page<Board>>(&format!("{}/board?projectKeyOrId={key}", api::AGILE))
                .await
                .ok()
                .and_then(|page| page.values.into_iter().next())
        }
    });
    provide_context(ProjectContext { key, board });
    view! {
        <div class="project-layout">
            <aside class="sidebar">
                {move || project.get().flatten().map(|p| view! {
                    <div class="sidebar-project">
                        <img class="avatar square" src=p.avatar() alt=""/>
                        <div>
                            <div class="sidebar-project-name">{p.name}</div>
                            <div class="muted small">{format!("{} project", p.project_type_key)}</div>
                        </div>
                    </div>
                })}
                <nav class="sidebar-nav">
                    {move || board.get().flatten().map(|b| {
                        let scrum = b.board_type == "scrum";
                        view! {
                            <Show when=move || scrum>
                                <A href="backlog" attr:class="sidebar-link">"Backlog"</A>
                            </Show>
                            <A href="board" attr:class="sidebar-link">{if scrum { "Active sprint" } else { "Board" }}</A>
                        }
                    })}
                    <A href="issues" attr:class="sidebar-link">"Issues"</A>
                </nav>
                {move || project.get().flatten().and_then(|p| p.lead).map(|lead| view! {
                    <div class="sidebar-footer muted small">
                        "Lead: " <Avatar user=Some(lead.clone()) size=16/> {lead.display_name}
                    </div>
                })}
            </aside>
            <section class="project-content">
                <Outlet/>
            </section>
        </div>
    }
}
