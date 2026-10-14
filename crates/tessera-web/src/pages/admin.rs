//! `/admin/users`: site administrators create, deactivate, and promote users.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct AdminUser {
    account_id: String,
    email_address: String,
    display_name: String,
    active: bool,
    admin: bool,
}

#[component]
pub fn Users() -> impl IntoView {
    let session = use_session();
    let users = LocalResource::new(move || {
        session.changes.track();
        async { api::get::<Vec<AdminUser>>(&format!("{}/admin/users", api::ACCOUNT)).await }
    });
    let creating = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // Sends one change for one user, such as `{"active": false}`.
    let change = move |account: String, body: Value| {
        spawn_local(async move {
            let path = format!("{}/admin/users/{account}", api::ACCOUNT);
            match api::put::<Value>(&path, &body).await {
                Ok(_) => {
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Users"</h1>
                <button class="button primary" on:click=move |_| creating.set(true)>"Create user"</button>
            </header>
            <ErrorText error=error/>
            <Transition fallback=|| view! { <p class="muted">"Loading users…"</p> }>
                {move || users.get().map(|result| match result {
                    Ok(list) => view! {
                        <table class="issue-table">
                            <thead><tr><th>"Name"</th><th>"Email"</th><th>"Status"</th><th>"Role"</th><th></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|u| {
                                    let (a1, a2) = (u.account_id.clone(), u.account_id.clone());
                                    let (active, admin) = (u.active, u.admin);
                                    view! {
                                        <tr>
                                            <td>{u.display_name}</td>
                                            <td class="muted">{u.email_address}</td>
                                            <td>{if active { "Active" } else { "Deactivated" }}</td>
                                            <td>{if admin { "Administrator" } else { "User" }}</td>
                                            <td>
                                                <button class="button subtle small"
                                                    on:click=move |_| change(a1.clone(), json!({ "active": !active }))>
                                                    {if active { "Deactivate" } else { "Activate" }}
                                                </button>
                                                <button class="button subtle small"
                                                    on:click=move |_| change(a2.clone(), json!({ "admin": !admin }))>
                                                    {if admin { "Remove admin" } else { "Make admin" }}
                                                </button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any(),
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
            <Show when=move || creating.get()>
                <CreateUser on_close=Callback::new(move |()| creating.set(false))/>
            </Show>
            <Groups/>
        </div>
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct GroupRef {
    name: String,
    group_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct GroupPage {
    values: Vec<GroupRef>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Member {
    account_id: String,
    display_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct MemberPage {
    values: Vec<Member>,
}

/// Groups and their members. Permission schemes and project roles can name groups.
#[component]
fn Groups() -> impl IntoView {
    let session = use_session();
    let groups = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<GroupPage>(&format!("{}/group/bulk?maxResults=1000", api::CORE))
                .await
                .map(|p| p.values)
                .unwrap_or_default()
        }
    });
    let users = crate::components::users_resource();
    let name = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "name": name.get() });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/group", api::CORE), &body).await {
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
        <header class="page-header group-header"><h2>"Groups"</h2></header>
        <form class="inline-form" on:submit=create>
            <input class="input" placeholder="New group name" required
                prop:value=move || name.get()
                on:input:target=move |ev| name.set(ev.target().value())/>
            <button class="button primary" type="submit">"Create group"</button>
        </form>
        <ErrorText error=error/>
        <div class="group-list">
            {move || groups.get().unwrap_or_default().into_iter().map(|group| view! {
                <GroupCard group=group users=users error=error/>
            }).collect_view()}
        </div>
    }
}

#[component]
fn GroupCard(
    group: GroupRef,
    users: LocalResource<Vec<crate::model::User>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = use_session();
    let id = StoredValue::new(group.group_id.clone());
    let members = LocalResource::new(move || {
        session.changes.track();
        async move {
            api::get::<MemberPage>(&format!(
                "{}/group/member?groupId={}&maxResults=1000",
                api::CORE,
                id.get_value()
            ))
            .await
            .map(|p| p.values)
            .unwrap_or_default()
        }
    });
    let picked = RwSignal::new(None::<String>);
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(account) = picked.get() else { return };
        spawn_local(async move {
            let path = format!("{}/group/user?groupId={}", api::CORE, id.get_value());
            match api::post::<Value>(&path, &json!({ "accountId": account })).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let remove = move |account: String| {
        spawn_local(async move {
            let path = format!(
                "{}/group/user?groupId={}&accountId={account}",
                api::CORE,
                id.get_value()
            );
            match api::delete(&path).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let delete = move |_| {
        spawn_local(async move {
            match api::delete(&format!("{}/group?groupId={}", api::CORE, id.get_value())).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <div class="sprint-header">
                <h3 class="list-title">{group.name}</h3>
                <span class="spacer"></span>
                <button class="button subtle small" on:click=delete>"Delete group"</button>
            </div>
            <ul class="ref-list">
                {move || members.get().unwrap_or_default().into_iter().map(|member| {
                    let account = member.account_id.clone();
                    view! {
                        <li class="ref-row">
                            <span class="grow">{member.display_name}</span>
                            <button class="button subtle small" on:click=move |_| remove(account.clone())>"Remove"</button>
                        </li>
                    }
                }).collect_view()}
            </ul>
            <form class="inline-form" on:submit=add>
                <crate::components::UserSelect users=users selected=None on_change=Callback::new(move |id| picked.set(id))/>
                <button class="button" type="submit">"Add member"</button>
            </form>
        </section>
    }
}

#[component]
fn CreateUser(on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let email = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let admin = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({
            "emailAddress": email.get(),
            "displayName": name.get(),
            "password": password.get(),
            "admin": admin.get(),
        });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/admin/users", api::ACCOUNT), &body).await {
                Ok(_) => {
                    session.changed();
                    on_close.run(());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Create user".into() on_close=on_close>
            <form class="form" on:submit=submit>
                <label>"Email"
                    <input class="input" type="email" required prop:value=move || email.get()
                        on:input:target=move |ev| email.set(ev.target().value())/>
                </label>
                <label>"Name"
                    <input class="input" prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())/>
                </label>
                <label>"First password"
                    <input class="input" type="password" required minlength="8" autocomplete="new-password"
                        prop:value=move || password.get()
                        on:input:target=move |ev| password.set(ev.target().value())/>
                </label>
                <ErrorText error=error/>
                <div class="form-actions">
                    <label class="check">
                        <input type="checkbox" prop:checked=move || admin.get()
                            on:change:target=move |ev| admin.set(ev.target().checked())/>
                        "Site administrator"
                    </label>
                    <button class="button primary" type="submit">"Create"</button>
                </div>
            </form>
        </Modal>
    }
}
