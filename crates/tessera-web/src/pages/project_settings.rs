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
            <Show when=move || use_session().admin.get()>
                <WorkflowScheme/>
                <SecurityScheme/>
            </Show>
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct SchemeRef {
    id: i64,
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct SchemePage {
    values: Vec<SchemeRef>,
}

/// The project's workflow scheme. Changing it moves issues to statuses of the new workflows.
#[component]
fn WorkflowScheme() -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let error = RwSignal::new(None::<String>);
    let loaded = LocalResource::new(move || {
        let key = project.key.get();
        session.changes.track();
        async move {
            let found: Value = api::get(&format!("{}/project/{key}", api::CORE))
                .await
                .unwrap_or_default();
            let id = found["id"].as_str().unwrap_or_default().to_string();
            let all: SchemePage = api::get(&format!("{}/workflowscheme", api::CORE))
                .await
                .unwrap_or_default();
            let current: Value = api::get(&format!(
                "{}/workflowscheme/project?projectId={id}",
                api::CORE
            ))
            .await
            .unwrap_or_default();
            let current_id = current["values"][0]["workflowScheme"]["id"]
                .as_i64()
                .unwrap_or_default();
            (id, all.values, current_id)
        }
    });
    let change = move |scheme: String| {
        let Some((project_id, _, _)) = loaded.get_untracked() else {
            return;
        };
        spawn_local(async move {
            let body = json!({ "workflowSchemeId": scheme, "projectId": project_id });
            match api::request(
                gloo_net::http::Method::PUT,
                &format!("{}/workflowscheme/project", api::CORE),
                Some(&body),
            )
            .await
            {
                Ok(_) => {
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Workflow"</h2>
            <p class="muted small">"Issues in a status the new workflow lacks move to a status of the same kind."</p>
            <select class="select" on:change:target=move |ev| change(ev.target().value())>
                {move || loaded.get().map(|(_, all, current)| all.into_iter().map(|s| {
                    let selected = s.id == current;
                    view! { <option value=s.id.to_string() selected=selected>{s.name}</option> }
                }).collect_view())}
            </select>
            <ErrorText error=error/>
        </section>
    }
}

/// The levels of a scheme, as (id, name).
async fn scheme_levels(id: &str) -> Vec<(String, String)> {
    if id.is_empty() {
        return Vec::new();
    }
    api::get::<Value>(&format!("{}/issuesecurityschemes/{id}", api::CORE))
        .await
        .ok()
        .and_then(|s| s["levels"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|l| {
            (
                l["id"].as_str().unwrap_or_default().to_string(),
                l["name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The project's issue security scheme. As in Jira, moving to another scheme asks where the
/// issues at each current level should go.
#[component]
fn SecurityScheme() -> impl IntoView {
    let session = use_session();
    let project = use_project();
    let error = RwSignal::new(None::<String>);
    let loaded = LocalResource::new(move || {
        let key = project.key.get();
        session.changes.track();
        async move {
            let found: Value = api::get(&format!("{}/project/{key}", api::CORE))
                .await
                .unwrap_or_default();
            let id = found["id"].as_str().unwrap_or_default().to_string();
            let all: Value = api::get(&format!("{}/issuesecurityschemes", api::CORE))
                .await
                .unwrap_or_default();
            let current: Value = api::get(&format!(
                "{}/issuesecurityschemes/project?projectId={id}",
                api::CORE
            ))
            .await
            .unwrap_or_default();
            let current_id = current["values"][0]["issueSecuritySchemeId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let schemes: Vec<(String, String)> = all["issueSecuritySchemes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    (
                        s["id"].to_string(),
                        s["name"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let current_levels = scheme_levels(&current_id).await;
            (id, schemes, current_id, current_levels)
        }
    });
    // The scheme picked but not saved yet, its levels, and where each current level goes.
    let pending = RwSignal::new(None::<(String, Vec<(String, String)>)>);
    let mapping = RwSignal::new(Vec::<(String, String)>::new());
    let save = move |scheme: String, mapping: Vec<(String, String)>| {
        let Some((project_id, _, _, _)) = loaded.get_untracked() else {
            return;
        };
        let scheme = if scheme.is_empty() {
            "-1".to_string()
        } else {
            scheme
        };
        let mappings: Vec<Value> = mapping
            .iter()
            .map(|(old, new)| json!({ "oldLevelId": old, "newLevelId": if new.is_empty() { "-1" } else { new.as_str() } }))
            .collect();
        spawn_local(async move {
            let body = json!({ "schemeId": scheme, "projectId": project_id, "oldToNewSecurityLevelMappings": mappings });
            match api::request(
                gloo_net::http::Method::PUT,
                &format!("{}/issuesecurityschemes/project", api::CORE),
                Some(&body),
            )
            .await
            {
                Ok(_) => {
                    error.set(None);
                    pending.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let pick = move |scheme: String| {
        let Some((_, _, current, current_levels)) = loaded.get_untracked() else {
            return;
        };
        if scheme == current {
            pending.set(None);
            return;
        }
        // Without current levels, or when removing the scheme, there is nothing to map.
        if current_levels.is_empty() || scheme.is_empty() {
            save(scheme, Vec::new());
            return;
        }
        spawn_local(async move {
            let levels = scheme_levels(&scheme).await;
            mapping.set(
                current_levels
                    .iter()
                    .map(|(id, _)| (id.clone(), String::new()))
                    .collect(),
            );
            pending.set(Some((scheme, levels)));
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Issue security"</h2>
            <p class="muted small">"Only members of an issue's security level see the issue."</p>
            <select class="select" aria-label="Issue security scheme"
                prop:value=move || pending.get().map(|(scheme, _)| scheme)
                    .or_else(|| loaded.get().map(|l| l.2))
                    .unwrap_or_default()
                on:change:target=move |ev| pick(ev.target().value())>
                {move || loaded.get().map(|(_, all, current, _)| view! {
                    <option value="" selected=current.is_empty()>"None"</option>
                    {all.into_iter().map(|(id, name)| {
                        let selected = id == current;
                        view! { <option value=id selected=selected>{name}</option> }
                    }).collect_view()}
                })}
            </select>
            {move || pending.get().map(|(scheme, levels)| {
                let current_levels = loaded.get().map(|l| l.3).unwrap_or_default();
                let scheme_for_save = scheme.clone();
                view! {
                    <div class="level-mapping">
                        <p class="small">"Pick a level in the new scheme for the issues at each current level."</p>
                        <table class="issue-table">
                            <thead><tr><th>"Current level"</th><th>"New level"</th></tr></thead>
                            <tbody>
                                {current_levels.into_iter().map(|(old, name)| {
                                    let levels = levels.clone();
                                    view! {
                                        <tr>
                                            <td>{name}</td>
                                            <td>
                                                <select class="select" aria-label="New level"
                                                    on:change:target=move |ev| {
                                                        let value = ev.target().value();
                                                        mapping.update(|m| {
                                                            if let Some(entry) = m.iter_mut().find(|(o, _)| *o == old) {
                                                                entry.1 = value;
                                                            }
                                                        });
                                                    }>
                                                    <option value="">"None"</option>
                                                    {levels.into_iter().map(|(id, name)| view! { <option value=id>{name}</option> }).collect_view()}
                                                </select>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        <div class="form-actions">
                            <button class="button primary" on:click=move |_| save(scheme_for_save.clone(), mapping.get())>"Change scheme"</button>
                            <button class="button subtle" on:click=move |_| { pending.set(None); session.changed(); }>"Cancel"</button>
                        </div>
                    </div>
                }
            })}
            <ErrorText error=error/>
        </section>
    }
}
