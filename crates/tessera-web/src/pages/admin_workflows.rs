//! `/admin/workflows`: statuses, workflows, and workflow schemes.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::ErrorText;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct StatusItem {
    id: String,
    name: String,
    status_category: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Paged<T> {
    values: Vec<T>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct WorkflowId {
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct WorkflowTransition {
    name: String,
    from: Vec<String>,
    to: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct WorkflowItem {
    id: WorkflowId,
    statuses: Vec<StatusItem>,
    transitions: Vec<WorkflowTransition>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct SchemeItem {
    name: String,
    default_workflow: String,
}

/// The tabs of the administration pages.
#[component]
pub fn AdminTabs(current: &'static str) -> impl IntoView {
    let tab = |href: &'static str, label: &'static str| {
        let class = if label == current {
            "tab active"
        } else {
            "tab"
        };
        view! { <a href=href class=class>{label}</a> }
    };
    view! {
        <nav class="tabs admin-tabs">
            {tab("/admin/users", "Users and groups")}
            {tab("/admin/workflows", "Workflows")}
            {tab("/admin/security", "Issue security")}
            {tab("/admin/import", "Import")}
        </nav>
    }
}

#[component]
pub fn Workflows() -> impl IntoView {
    let session = use_session();
    let statuses = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<Paged<StatusItem>>(&format!("{}/statuses/search?maxResults=200", api::CORE))
                .await
                .map(|p| p.values)
                .unwrap_or_default()
        }
    });
    let workflows = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<Paged<WorkflowItem>>(&format!("{}/workflow/search?maxResults=50", api::CORE))
                .await
                .map(|p| p.values)
                .unwrap_or_default()
        }
    });
    let schemes = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<Paged<SchemeItem>>(&format!("{}/workflowscheme", api::CORE))
                .await
                .map(|p| p.values)
                .unwrap_or_default()
        }
    });
    view! {
        <div class="page settings-page wide-settings">
            <header class="page-header"><h1>"Administration"</h1></header>
            <AdminTabs current="Workflows"/>
            <Statuses statuses=statuses/>
            <section class="settings-card">
                <h2>"Workflows"</h2>
                {move || workflows.get().unwrap_or_default().into_iter().map(|w| view! { <WorkflowCard workflow=w/> }).collect_view()}
                <NewWorkflow statuses=statuses/>
            </section>
            <Schemes schemes=schemes workflows=workflows/>
        </div>
    }
}

#[component]
fn Statuses(statuses: LocalResource<Vec<StatusItem>>) -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(String::new());
    let category = RwSignal::new("TODO".to_string());
    let error = RwSignal::new(None::<String>);
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "scope": { "type": "GLOBAL" }, "statuses": [{ "name": name.get(), "statusCategory": category.get() }] });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/statuses", api::CORE), &body).await {
                Ok(_) => {
                    name.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Statuses"</h2>
            <div class="chips">
                {move || statuses.get().unwrap_or_default().into_iter().map(|s| {
                    let class = match s.status_category.as_str() {
                        "DONE" => "lozenge lozenge-done",
                        "IN_PROGRESS" => "lozenge lozenge-indeterminate",
                        _ => "lozenge lozenge-new",
                    };
                    view! { <span class=class>{s.name}</span> }
                }).collect_view()}
            </div>
            <form class="inline-form" on:submit=create>
                <input class="input" placeholder="New status, such as In Review" required
                    prop:value=move || name.get()
                    on:input:target=move |ev| name.set(ev.target().value())/>
                <select class="select link-type" on:change:target=move |ev| category.set(ev.target().value())>
                    <option value="TODO">"To do"</option>
                    <option value="IN_PROGRESS">"In progress"</option>
                    <option value="DONE">"Done"</option>
                </select>
                <button class="button primary" type="submit">"Add status"</button>
            </form>
            <ErrorText error=error/>
        </section>
    }
}

#[component]
fn WorkflowCard(workflow: WorkflowItem) -> impl IntoView {
    let name_of = {
        let statuses = workflow.statuses.clone();
        move |id: &str| {
            statuses
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| id.to_string())
        }
    };
    let rows: Vec<(String, String, String)> = workflow
        .transitions
        .iter()
        .filter(|t| t.kind != "initial")
        .map(|t| {
            let from = if t.from.is_empty() {
                "Any status".to_string()
            } else {
                t.from
                    .iter()
                    .map(|f| name_of(f))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            (t.name.clone(), from, name_of(&t.to))
        })
        .collect();
    let initial = workflow
        .transitions
        .iter()
        .find(|t| t.kind == "initial")
        .map(|t| name_of(&t.to))
        .unwrap_or_default();
    view! {
        <details class="workflow-card">
            <summary>
                <strong>{workflow.id.name.clone()}</strong>
                <span class="muted small">{format!(" · {} statuses · starts at {initial}", workflow.statuses.len())}</span>
            </summary>
            <table class="issue-table">
                <thead><tr><th>"Transition"</th><th>"From"</th><th>"To"</th></tr></thead>
                <tbody>
                    {rows.into_iter().map(|(name, from, to)| view! {
                        <tr><td>{name}</td><td class="muted">{from}</td><td>{to}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
        </details>
    }
}

/// One transition being drafted: name, source status (empty for any), and target.
#[derive(Clone, Default)]
struct DraftTransition {
    name: String,
    from: String,
    to: String,
}

#[component]
fn NewWorkflow(statuses: LocalResource<Vec<StatusItem>>) -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(String::new());
    let chosen = RwSignal::new(Vec::<String>::new());
    let initial = RwSignal::new(String::new());
    let drafts = RwSignal::new(Vec::<DraftTransition>::new());
    let draft = RwSignal::new(DraftTransition::default());
    let error = RwSignal::new(None::<String>);
    let status_name = move |id: &str| {
        statuses
            .get()
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.name)
            .unwrap_or_default()
    };
    let add_transition = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let next = draft.get();
        if next.name.trim().is_empty() || next.to.is_empty() {
            return;
        }
        drafts.update(|list| list.push(next));
        draft.set(DraftTransition::default());
    };
    let save = move |_| {
        let picked = chosen.get();
        let start = if initial.get().is_empty() {
            picked.first().cloned().unwrap_or_default()
        } else {
            initial.get()
        };
        let mut transitions = vec![json!({ "name": "Create", "to": start, "type": "initial" })];
        for t in drafts.get() {
            transitions.push(if t.from.is_empty() {
                json!({ "name": t.name, "to": t.to, "type": "global" })
            } else {
                json!({ "name": t.name, "from": [t.from], "to": t.to, "type": "directed" })
            });
        }
        let body = json!({
            "name": name.get(),
            "statuses": picked.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "transitions": transitions,
        });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/workflow", api::CORE), &body).await {
                Ok(_) => {
                    name.set(String::new());
                    chosen.set(Vec::new());
                    drafts.set(Vec::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let options = move || {
        chosen
            .get()
            .into_iter()
            .map(|id| {
                let label = status_name(&id);
                view! { <option value=id>{label}</option> }
            })
            .collect_view()
    };
    view! {
        <details class="workflow-card new-workflow">
            <summary><strong>"New workflow"</strong></summary>
            <div class="form">
                <label>"Name"
                    <input class="input" prop:value=move || name.get() on:input:target=move |ev| name.set(ev.target().value())/>
                </label>
                <fieldset class="choices">
                    <legend>"Statuses"</legend>
                    <div class="chips">
                        {move || statuses.get().unwrap_or_default().into_iter().map(|s| {
                            let id = s.id.clone();
                            let checked = chosen.get().contains(&id);
                            view! {
                                <label class="check-chip">
                                    <input type="checkbox" prop:checked=checked on:change=move |_| {
                                        let id = id.clone();
                                        chosen.update(|list| {
                                            if let Some(i) = list.iter().position(|x| *x == id) { list.remove(i); } else { list.push(id); }
                                        });
                                    }/>
                                    {s.name}
                                </label>
                            }
                        }).collect_view()}
                    </div>
                </fieldset>
                <label>"New issues start at"
                    <select class="select" on:change:target=move |ev| initial.set(ev.target().value())>
                        <option value="">"The first status"</option>
                        {options}
                    </select>
                </label>
                <h3>"Transitions"</h3>
                <ul class="ref-list">
                    {move || drafts.get().into_iter().map(|t| {
                        let from = if t.from.is_empty() { "Any status".to_string() } else { status_name(&t.from) };
                        view! { <li class="ref-row">{format!("{}: {} → {}", t.name, from, status_name(&t.to))}</li> }
                    }).collect_view()}
                </ul>
                <form class="inline-form" on:submit=add_transition>
                    <input class="input" placeholder="Transition name" prop:value=move || draft.get().name
                        on:input:target=move |ev| draft.update(|d| d.name = ev.target().value())/>
                    <select class="select link-type" prop:value=move || draft.get().from
                        on:change:target=move |ev| draft.update(|d| d.from = ev.target().value())>
                        <option value="">"From any status"</option>
                        {options}
                    </select>
                    <select class="select link-type" prop:value=move || draft.get().to
                        on:change:target=move |ev| draft.update(|d| d.to = ev.target().value())>
                        <option value="">"To…"</option>
                        {options}
                    </select>
                    <button class="button" type="submit">"Add"</button>
                </form>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" on:click=save>"Create workflow"</button>
                </div>
            </div>
        </details>
    }
}

#[component]
fn Schemes(
    schemes: LocalResource<Vec<SchemeItem>>,
    workflows: LocalResource<Vec<WorkflowItem>>,
) -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(String::new());
    let default = RwSignal::new("Simplified Workflow".to_string());
    let error = RwSignal::new(None::<String>);
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "name": name.get(), "defaultWorkflow": default.get() });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/workflowscheme", api::CORE), &body).await {
                Ok(_) => {
                    name.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Workflow schemes"</h2>
            <p class="muted small">"A project uses one scheme. Pick it in the project's settings."</p>
            <table class="issue-table">
                <thead><tr><th>"Scheme"</th><th>"Default workflow"</th></tr></thead>
                <tbody>
                    {move || schemes.get().unwrap_or_default().into_iter().map(|s| view! {
                        <tr><td>{s.name}</td><td>{s.default_workflow}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
            <form class="inline-form" on:submit=create>
                <input class="input" placeholder="Scheme name" required prop:value=move || name.get()
                    on:input:target=move |ev| name.set(ev.target().value())/>
                <select class="select link-type" on:change:target=move |ev| default.set(ev.target().value())>
                    {move || workflows.get().unwrap_or_default().into_iter().map(|w| {
                        let label = w.id.name.clone();
                        let selected = w.id.name == default.get_untracked();
                        view! { <option value=w.id.name selected=selected>{label}</option> }
                    }).collect_view()}
                </select>
                <button class="button primary" type="submit">"Create scheme"</button>
            </form>
            <ErrorText error=error/>
        </section>
    }
}
