//! The create issue dialog, opened from the top navigation.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal, UserSelect, users_resource};
use crate::model::{Named, Project};

#[component]
pub fn CreateIssue() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let close = Callback::new(move |()| session.creating.set(false));
    let projects = LocalResource::new(|| async {
        api::get::<Vec<Project>>(&format!("{}/project", api::CORE))
            .await
            .unwrap_or_default()
    });
    let issue_types = LocalResource::new(|| async {
        api::get::<Vec<Named>>(&format!("{}/issuetype", api::CORE))
            .await
            .unwrap_or_default()
    });
    let priorities = LocalResource::new(|| async {
        api::get::<Vec<Named>>(&format!("{}/priority", api::CORE))
            .await
            .unwrap_or_default()
    });
    let users = users_resource();

    let project = RwSignal::new(String::new());
    let issue_type = RwSignal::new("Task".to_string());
    let summary = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let assignee = RwSignal::new(None::<String>);
    let priority = RwSignal::new("Medium".to_string());
    let labels = RwSignal::new(String::new());
    let another = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let created = RwSignal::new(None::<String>);

    // Default to the first project once the list arrives.
    Effect::new(move |_| {
        if project.get_untracked().is_empty()
            && let Some(first) = projects.get().and_then(|list| list.into_iter().next())
        {
            project.set(first.key);
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let mut fields = json!({
            "project": { "key": project.get() },
            "issuetype": { "name": issue_type.get() },
            "summary": summary.get(),
            "priority": { "name": priority.get() },
            "labels": labels.get().split_whitespace().collect::<Vec<_>>(),
        });
        let text = description.get();
        if !text.trim().is_empty() {
            fields["description"] =
                serde_json::to_value(tessera_adf::from_markdown(&text)).unwrap_or(Value::Null);
        }
        if let Some(id) = assignee.get() {
            fields["assignee"] = json!({ "accountId": id });
        }
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::post::<Value>(
                &format!("{}/issue", api::CORE),
                &json!({ "fields": fields }),
            )
            .await
            {
                Ok(issue) => {
                    let key = issue["key"].as_str().unwrap_or_default().to_string();
                    session.changed();
                    error.set(None);
                    if another.get_untracked() {
                        summary.set(String::new());
                        description.set(String::new());
                        created.set(Some(key));
                    } else {
                        session.creating.set(false);
                        navigate(&format!("/browse/{key}"), Default::default());
                    }
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };

    view! {
        <Modal title="Create issue".into() on_close=close>
            <form class="form" on:submit=submit>
                <div class="form-row">
                    <label>"Project"
                        <select class="select" required on:change:target=move |ev| project.set(ev.target().value())>
                            {move || projects.get().unwrap_or_default().into_iter().map(|p| {
                                let selected = p.key == project.get();
                                view! { <option value=p.key.clone() selected=selected>{format!("{} ({})", p.name, p.key)}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                    <label>"Issue type"
                        <select class="select" on:change:target=move |ev| issue_type.set(ev.target().value())>
                            {move || issue_types.get().unwrap_or_default().into_iter().filter(|t| !t.subtask).map(|t| {
                                let selected = t.name == issue_type.get();
                                let label = t.name.clone();
                                view! { <option value=t.name selected=selected>{label}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                </div>
                <label>"Summary"
                    <input class="input" required maxlength="255"
                        prop:value=move || summary.get()
                        on:input:target=move |ev| summary.set(ev.target().value())/>
                </label>
                <label>"Description"
                    <textarea class="input" rows="6" placeholder="Markdown works: **bold**, *italic*, lists, `code`."
                        prop:value=move || description.get()
                        on:input:target=move |ev| description.set(ev.target().value())></textarea>
                </label>
                <div class="form-row">
                    <label>"Assignee"
                        <UserSelect users=users selected=None on_change=Callback::new(move |id| assignee.set(id))/>
                    </label>
                    <label>"Priority"
                        <select class="select" on:change:target=move |ev| priority.set(ev.target().value())>
                            {move || priorities.get().unwrap_or_default().into_iter().map(|p| {
                                let selected = p.name == priority.get();
                                let label = p.name.clone();
                                view! { <option value=p.name selected=selected>{label}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                </div>
                <label>"Labels"
                    <input class="input" placeholder="Separate with spaces"
                        prop:value=move || labels.get()
                        on:input:target=move |ev| labels.set(ev.target().value())/>
                </label>
                <ErrorText error=error/>
                {move || created.get().map(|key| view! { <p class="notice">{format!("Created {key}.")}</p> })}
                <div class="form-actions">
                    <label class="check">
                        <input type="checkbox" prop:checked=move || another.get()
                            on:change:target=move |ev| another.set(ev.target().checked())/>
                        "Create another"
                    </label>
                    <button class="button primary" type="submit">"Create"</button>
                </div>
            </form>
        </Modal>
    }
}
