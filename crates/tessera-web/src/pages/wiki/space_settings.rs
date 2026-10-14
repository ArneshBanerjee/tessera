//! `/wiki/spaces/:key/settings`: a space's details and permissions.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::use_space;
use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, UserSelect, users_resource};

/// The operations shown as columns: (operation, target, heading).
const COLUMNS: &[(&str, &str, &str)] = &[
    ("read", "space", "View"),
    ("create", "page", "Add pages"),
    ("create", "blogpost", "Add blogs"),
    ("create", "comment", "Comment"),
    ("create", "attachment", "Attach"),
    ("delete", "space", "Delete own"),
    ("delete", "page", "Delete pages"),
    ("delete", "comment", "Delete comments"),
    ("delete", "attachment", "Delete files"),
    ("administer", "space", "Admin"),
];

/// Who a permission is for, as shown in a row: (type, identifier, label).
type Subject = (String, String, String);

/// Permission ids by subject and by (operation, target).
type Grid = BTreeMap<Subject, BTreeMap<(String, String), i64>>;

fn grid_of(space: &Value) -> Grid {
    let mut grid = Grid::new();
    for permission in space["permissions"].as_array().into_iter().flatten() {
        let subject = if let Some(user) = permission["subjects"]["user"]["results"].get(0) {
            (
                "user".to_string(),
                user["accountId"].as_str().unwrap_or_default().to_string(),
                user["displayName"].as_str().unwrap_or_default().to_string(),
            )
        } else if let Some(group) = permission["subjects"]["group"]["results"].get(0) {
            let name = group["name"].as_str().unwrap_or_default().to_string();
            ("group".to_string(), name.clone(), format!("{name} (group)"))
        } else if permission["anonymousAccess"] == true {
            (
                "anonymous".to_string(),
                String::new(),
                "Anonymous".to_string(),
            )
        } else {
            continue;
        };
        let operation = permission["operation"]["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let target = permission["operation"]["targetType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let id = permission["id"].as_i64().unwrap_or_default();
        grid.entry(subject)
            .or_default()
            .insert((operation, target), id);
    }
    grid
}

#[component]
pub fn SpaceSettings() -> impl IntoView {
    let session = use_session();
    let space = use_space();
    let error = RwSignal::new(None::<String>);
    let loaded = LocalResource::new(move || {
        let key = space.key.get();
        session.changes.track();
        async move {
            api::get::<Value>(&format!(
                "{}/space/{key}?expand=permissions,description.plain",
                api::WIKI
            ))
            .await
        }
    });
    let grant = move |subject: Subject, operation: &'static str, target: &'static str| {
        let key = space.key.get_untracked();
        spawn_local(async move {
            let body = json!({
                "subject": { "type": subject.0, "identifier": subject.1 },
                "operation": { "key": operation, "target": target },
            });
            match api::post::<Value>(&format!("{}/space/{key}/permission", api::WIKI), &body).await
            {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let revoke = move |id: i64| {
        let key = space.key.get_untracked();
        spawn_local(async move {
            match api::delete(&format!("{}/space/{key}/permission/{id}", api::WIKI)).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="page settings-page wide-settings">
            <header class="page-header"><h1>"Space settings"</h1></header>
            <ErrorText error=error/>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || loaded.get().map(|result| match result {
                    Ok(found) => {
                        let grid = grid_of(&found);
                        view! {
                            <Details found=found.clone()/>
                            <section class="settings-card">
                                <h2>"Permissions"</h2>
                                <p class="muted small">"Each row is a user or group; each box allows one thing in this space."</p>
                                <div class="table-scroll">
                                    <table class="issue-table permission-grid">
                                        <thead><tr>
                                            <th>"Who"</th>
                                            {COLUMNS.iter().map(|(_, _, heading)| view! { <th>{*heading}</th> }).collect_view()}
                                        </tr></thead>
                                        <tbody>
                                            {grid.into_iter().map(|(subject, granted)| {
                                                let label = subject.2.clone();
                                                view! {
                                                    <tr>
                                                        <td>{label}</td>
                                                        {COLUMNS.iter().map(|(operation, target, _)| {
                                                            let id = granted.get(&(operation.to_string(), target.to_string())).copied();
                                                            let subject = subject.clone();
                                                            view! {
                                                                <td class="check-cell">
                                                                    <input type="checkbox" prop:checked=id.is_some()
                                                                        on:change=move |_| match id {
                                                                            Some(id) => revoke(id),
                                                                            None => grant(subject.clone(), operation, target),
                                                                        }/>
                                                                </td>
                                                            }
                                                        }).collect_view()}
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                                <AddSubject on_add=Callback::new(move |subject: Subject| grant(subject, "read", "space"))/>
                            </section>
                        }.into_any()
                    }
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[component]
fn Details(found: Value) -> impl IntoView {
    let session = use_session();
    let space = use_space();
    let name = RwSignal::new(found["name"].as_str().unwrap_or_default().to_string());
    let description = RwSignal::new(
        found["description"]["plain"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    let error = RwSignal::new(None::<String>);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let key = space.key.get_untracked();
        let body = json!({
            "name": name.get(),
            "description": { "plain": { "value": description.get(), "representation": "plain" } },
        });
        spawn_local(async move {
            match api::put::<Value>(&format!("{}/space/{key}", api::WIKI), &body).await {
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
            <h2>"Details"</h2>
            <form class="form" on:submit=save>
                <label>"Name"
                    <input class="input" required prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())/>
                </label>
                <label>"Description"
                    <textarea class="input" rows="3" prop:value=move || description.get()
                        on:input:target=move |ev| description.set(ev.target().value())></textarea>
                </label>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" type="submit">"Save"</button>
                </div>
            </form>
        </section>
    }
}

/// Adds a user or a group as a new row, starting with permission to view the space.
#[component]
fn AddSubject(on_add: Callback<Subject>) -> impl IntoView {
    let users = users_resource();
    let picked = RwSignal::new(None::<String>);
    let group = RwSignal::new(String::new());
    let add_user = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if let Some(account) = picked.get() {
            on_add.run(("user".into(), account, String::new()));
        }
    };
    let add_group = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name = group.get().trim().to_string();
        if !name.is_empty() {
            on_add.run(("group".into(), name, String::new()));
            group.set(String::new());
        }
    };
    view! {
        <div class="add-subjects">
            <form class="inline-form" on:submit=add_user>
                <UserSelect users=users selected=None on_change=Callback::new(move |id| picked.set(id))/>
                <button class="button" type="submit">"Add user"</button>
            </form>
            <form class="inline-form" on:submit=add_group>
                <input class="input" placeholder="Group name" prop:value=move || group.get()
                    on:input:target=move |ev| group.set(ev.target().value())/>
                <button class="button" type="submit">"Add group"</button>
            </form>
        </div>
    }
}
