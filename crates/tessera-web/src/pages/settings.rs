//! `/settings`: the signed-in user's profile, password, and API tokens.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::ErrorText;
use crate::model::short_time;

#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <div class="page settings-page">
            <header class="page-header"><h1>"Your profile"</h1></header>
            <Profile/>
            <Password/>
            <Tokens/>
        </div>
    }
}

#[component]
fn Profile() -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(session.me.get_value().display_name);
    let zone = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(me) = api::get::<Value>(&format!("{}/me", api::ACCOUNT)).await {
            zone.set(me["timeZone"].as_str().unwrap_or("UTC").to_string());
        }
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "displayName": name.get(), "timeZone": zone.get() });
        spawn_local(async move {
            match api::put::<Value>(&format!("{}/profile", api::ACCOUNT), &body).await {
                Ok(_) => {
                    error.set(None);
                    saved.set(true);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Profile"</h2>
            <form class="form" on:submit=save>
                <div class="form-row">
                    <label>"Name"
                        <input class="input" required prop:value=move || name.get()
                            on:input:target=move |ev| { saved.set(false); name.set(ev.target().value()) }/>
                    </label>
                    <label>"Time zone"
                        <input class="input" placeholder="For example Europe/Berlin" prop:value=move || zone.get()
                            on:input:target=move |ev| { saved.set(false); zone.set(ev.target().value()) }/>
                    </label>
                </div>
                <ErrorText error=error/>
                <div class="form-actions">
                    <Show when=move || saved.get()><span class="notice">"Saved."</span></Show>
                    <button class="button primary" type="submit">"Save"</button>
                </div>
            </form>
        </section>
    }
}

#[component]
fn Password() -> impl IntoView {
    let current = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "currentPassword": current.get(), "newPassword": new.get() });
        spawn_local(async move {
            match api::request(
                gloo_net::http::Method::PUT,
                &format!("{}/password", api::ACCOUNT),
                Some(&body),
            )
            .await
            {
                Ok(_) => {
                    error.set(None);
                    current.set(String::new());
                    new.set(String::new());
                    done.set(true);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"Password"</h2>
            <p class="muted small">"Changing your password signs you out everywhere else."</p>
            <form class="form" on:submit=save>
                <div class="form-row">
                    <label>"Current password"
                        <input class="input" type="password" autocomplete="current-password"
                            prop:value=move || current.get()
                            on:input:target=move |ev| current.set(ev.target().value())/>
                    </label>
                    <label>"New password"
                        <input class="input" type="password" autocomplete="new-password" required minlength="8"
                            prop:value=move || new.get()
                            on:input:target=move |ev| { done.set(false); new.set(ev.target().value()) }/>
                    </label>
                </div>
                <ErrorText error=error/>
                <div class="form-actions">
                    <Show when=move || done.get()><span class="notice">"Password changed."</span></Show>
                    <button class="button primary" type="submit">"Change password"</button>
                </div>
            </form>
        </section>
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Token {
    id: String,
    label: String,
    created: String,
    last_used: Option<String>,
}

#[component]
fn Tokens() -> impl IntoView {
    let session = use_session();
    let tokens = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<Vec<Token>>(&format!("{}/api-tokens", api::ACCOUNT))
                .await
                .unwrap_or_default()
        }
    });
    let label = RwSignal::new(String::new());
    // A new secret, shown until the page is left. Tessera cannot show it again.
    let secret = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "label": label.get() });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/api-tokens", api::ACCOUNT), &body).await {
                Ok(created) => {
                    secret.set(created["token"].as_str().map(str::to_string));
                    label.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let revoke = move |id: String| {
        spawn_local(async move {
            match api::delete(&format!("{}/api-tokens/{id}", api::ACCOUNT)).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>"API tokens"</h2>
            <p class="muted small">
                "Scripts and clients sign in with your email and a token, the same way they do with Atlassian Cloud."
            </p>
            {move || secret.get().map(|token| view! {
                <div class="notice-bar token-bar">
                    <span>"Copy your new token now. It will not be shown again."</span>
                    <code class="token">{token}</code>
                </div>
            })}
            <form class="inline-form" on:submit=create>
                <input class="input" placeholder="Label, such as CI or laptop" required
                    prop:value=move || label.get()
                    on:input:target=move |ev| label.set(ev.target().value())/>
                <button class="button primary" type="submit">"Create token"</button>
            </form>
            <ErrorText error=error/>
            <table class="issue-table">
                <thead><tr><th>"Label"</th><th>"Created"</th><th>"Last used"</th><th></th></tr></thead>
                <tbody>
                    {move || tokens.get().unwrap_or_default().into_iter().map(|t| {
                        let id = t.id.clone();
                        view! {
                            <tr>
                                <td>{t.label}</td>
                                <td class="muted">{short_time(&t.created)}</td>
                                <td class="muted">{t.last_used.map(|u| short_time(&u)).unwrap_or_else(|| "Never".into())}</td>
                                <td><button class="button subtle small" on:click=move |_| revoke(id.clone())>"Revoke"</button></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}
