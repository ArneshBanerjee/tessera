//! The notification bell in the top bar: what happened to the issues and pages you follow,
//! and where people mentioned you.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde::Deserialize;

use crate::api;
use crate::app::use_session;
use crate::components::Avatar;
use crate::model::{User, short_time};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Note {
    id: String,
    kind: String,
    title: String,
    body: String,
    link: String,
    actor: Option<User>,
    created: String,
    read: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Inbox {
    unread: i64,
    values: Vec<Note>,
}

#[component]
pub fn NotificationBell() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let open = RwSignal::new(false);
    // Checks for new notifications every minute, and after the user changes something.
    let tick = RwSignal::new(0u32);
    let _ = set_interval_with_handle(move || tick.update(|t| *t += 1), Duration::from_secs(60));
    let inbox = LocalResource::new(move || {
        tick.track();
        session.changes.track();
        async move {
            api::get::<Inbox>(&format!("{}/notifications?maxResults=30", api::ACCOUNT))
                .await
                .unwrap_or_default()
        }
    });
    let unread = move || inbox.get().map(|i| i.unread).unwrap_or(0);
    let read_all = move |_| {
        spawn_local(async move {
            let path = format!("{}/notifications/read-all", api::ACCOUNT);
            if api::request(gloo_net::http::Method::POST, &path, None)
                .await
                .is_ok()
            {
                tick.update(|t| *t += 1);
            }
        });
    };
    let follow = Callback::new(move |note: Note| {
        open.set(false);
        let navigate = navigate.clone();
        spawn_local(async move {
            if !note.read {
                let path = format!("{}/notifications/{}/read", api::ACCOUNT, note.id);
                let _ = api::request(gloo_net::http::Method::POST, &path, None).await;
                tick.update(|t| *t += 1);
            }
            navigate(&note.link, Default::default());
        });
    });
    view! {
        <div class="dropdown bell">
            <button class="icon-button bell-button" aria-label="Notifications" title="Notifications"
                on:click=move |_| open.update(|o| *o = !*o)>
                <span aria-hidden="true">"🔔"</span>
                {move || (unread() > 0).then(|| {
                    let count = unread();
                    view! { <span class="bell-count">{if count > 99 { "99+".to_string() } else { count.to_string() }}</span> }
                })}
            </button>
            <Show when=move || open.get()>
                <div class="menu bell-menu" role="dialog" aria-label="Notifications">
                    <header class="bell-header">
                        <strong>"Notifications"</strong>
                        <button class="link-button small" on:click=read_all>"Mark all as read"</button>
                    </header>
                    {move || {
                        let notes = inbox.get().map(|i| i.values).unwrap_or_default();
                        if notes.is_empty() {
                            return view! { <p class="muted small bell-empty">"Nothing yet. You hear about issues and pages you follow, and where people mention you."</p> }.into_any();
                        }
                        view! {
                            <ul class="bell-list">
                                {notes.into_iter().map(|note| {
                                    let chosen = note.clone();
                                    view! {
                                        <li class="bell-item" class:unread=!note.read class:kind-mention=note.kind == "mention"
                                            on:click=move |_| follow.run(chosen.clone())>
                                            <Avatar user=note.actor.clone() size=28/>
                                            <div class="bell-text">
                                                <span>{note.title.clone()}</span>
                                                {(!note.body.is_empty()).then(|| view! { <span class="muted small bell-body">{note.body.clone()}</span> })}
                                                <span class="muted small">{short_time(&note.created)}</span>
                                            </div>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}
                </div>
            </Show>
        </div>
    }
}
