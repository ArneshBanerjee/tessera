//! `/projects/:key/settings`: a project's details and who holds its roles.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::projects::use_project;
use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, UserSelect, users_resource};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct RoleRef {
    id: i64,
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Actor {
    display_name: String,
    #[serde(rename = "type")]
    kind: String,
    actor_user: Value,
    actor_group: Value,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct RoleWithActors {
    actors: Vec<Actor>,
}

#[component]
pub fn ProjectSettings() -> impl IntoView {
    view! {
        <div class="page settings-page">
            <header class="page-header"><h1>"Project settings"</h1></header>
            <Details/>
            <People/>
        </div>
    }
}

#[component]
fn Details() -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(false);
    Effect::new(move |_| {
        let key = project.key.get();
        spawn_local(async move {
            if let Ok(found) = api::get::<Value>(&format!("{}/project/{key}", api::CORE)).await {
                name.set(found["name"].as_str().unwrap_or_default().to_string());
                description.set(
                    found["description"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        });
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let key = project.key.get();
        let body = json!({ "name": name.get(), "description": description.get() });
        spawn_local(async move {
            match api::put::<Value>(&format!("{}/project/{key}", api::CORE), &body).await {
                Ok(_) => {
                    error.set(None);
                    saved.set(true);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Details"</h2>
            <form class="form" on:submit=save>
                <label>"Name"
                    <input class="input" required prop:value=move || name.get()
                        on:input:target=move |ev| { saved.set(false); name.set(ev.target().value()) }/>
                </label>
                <label>"Description"
                    <textarea class="input" rows="3" prop:value=move || description.get()
                        on:input:target=move |ev| { saved.set(false); description.set(ev.target().value()) }></textarea>
                </label>
                <ErrorText error=error/>
                <div class="form-actions">
                    <Show when=move || saved.get()><span class="notice">"Saved."</span></Show>
                    <button class="button primary" type="submit">"Save"</button>
                </div>
            </form>
        </section>
    }
}

/// Who holds each project role. Roles decide what people may do, through the permission scheme.
#[component]
fn People() -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let users = users_resource();
    let roles = LocalResource::new(move || {
        let key = project.key.get();
        session.changes.track();
        async move {
            let list: Vec<RoleRef> = api::get(&format!("{}/project/{key}/roledetails", api::CORE))
                .await
                .unwrap_or_default();
            let mut out = Vec::with_capacity(list.len());
            for role in list {
                if let Ok(full) = api::get::<RoleWithActors>(&format!(
                    "{}/project/{key}/role/{}",
                    api::CORE,
                    role.id
                ))
                .await
                {
                    out.push((role, full));
                }
            }
            out
        }
    });
    let picked_user = RwSignal::new(None::<String>);
    let picked_role = RwSignal::new(10003i64);
    let error = RwSignal::new(None::<String>);
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(account) = picked_user.get() else {
            return;
        };
        let key = project.key.get();
        let role = picked_role.get();
        spawn_local(async move {
            let path = format!("{}/project/{key}/role/{role}", api::CORE);
            match api::post::<Value>(&path, &json!({ "user": [account] })).await {
                Ok(_) => {
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let remove = move |role: i64, query: String| {
        let key = project.key.get();
        spawn_local(async move {
            match api::delete(&format!("{}/project/{key}/role/{role}?{query}", api::CORE)).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"People"</h2>
            <p class="muted small">"Project roles decide what people may do here. Administrators manage the project."</p>
            <form class="inline-form" on:submit=add>
                <UserSelect users=users selected=None on_change=Callback::new(move |id| picked_user.set(id))/>
                <select class="select link-type" on:change:target=move |ev| {
                    picked_role.set(ev.target().value().parse().unwrap_or(10003))
                }>
                    {move || roles.get().unwrap_or_default().into_iter().map(|(role, _)| {
                        let selected = role.id == picked_role.get_untracked();
                        view! { <option value=role.id.to_string() selected=selected>{role.name}</option> }
                    }).collect_view()}
                </select>
                <button class="button primary" type="submit">"Add"</button>
            </form>
            <ErrorText error=error/>
            <table class="issue-table">
                <thead><tr><th>"Name"</th><th>"Role"</th><th></th></tr></thead>
                <tbody>
                    {move || roles.get().unwrap_or_default().into_iter().flat_map(|(role, full)| {
                        full.actors.into_iter().map(move |actor| {
                            let query = if actor.kind == "atlassian-user-role-actor" {
                                format!("user={}", actor.actor_user["accountId"].as_str().unwrap_or_default())
                            } else {
                                format!("groupId={}", actor.actor_group["groupId"].as_str().unwrap_or_default())
                            };
                            let label = if actor.kind == "atlassian-group-role-actor" {
                                format!("{} (group)", actor.display_name)
                            } else {
                                actor.display_name.clone()
                            };
                            let role_id = role.id;
                            view! {
                                <tr>
                                    <td>{label}</td>
                                    <td>{role.name.clone()}</td>
                                    <td><button class="button subtle small" on:click=move |_| remove(role_id, query.clone())>"Remove"</button></td>
                                </tr>
                            }
                        }).collect::<Vec<_>>()
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}
