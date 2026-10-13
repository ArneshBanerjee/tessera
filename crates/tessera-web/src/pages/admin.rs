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
        </div>
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
