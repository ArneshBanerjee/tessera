//! `/wiki/spaces/:key/pages/:id`: a page, its labels, comments, and history.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use serde_json::{Value, json};

use super::{adf_body, page_path, use_space};
use crate::api;
use crate::app::use_session;
use crate::components::{Avatar, ErrorText};
use crate::model::{WikiCollection, WikiContent, WikiVersion, short_time};

/// What the page view asks the v1 API to include.
const PAGE_EXPAND: &str = "body.view,version,history,ancestors,space,metadata.labels";

#[component]
pub fn PageView() -> impl IntoView {
    let session = use_session();
    let params = use_params_map();
    let query = use_query_map();
    let id = Signal::derive(move || params.with(|p| p.get("id")).unwrap_or_default());
    // `?version=N` shows an older version.
    let version = Signal::derive(move || {
        query
            .with(|q| q.get("version"))
            .and_then(|v| v.parse::<i64>().ok())
    });
    let page = LocalResource::new(move || {
        let id = id.get();
        let version = version.get();
        session.changes.track();
        async move {
            let old = version.map(|v| format!("&version={v}")).unwrap_or_default();
            api::get::<WikiContent>(&format!(
                "{}/content/{id}?expand={PAGE_EXPAND}{old}",
                api::WIKI
            ))
            .await
        }
    });
    view! {
        <div class="page wiki-page">
            <Transition fallback=|| view! { <p class="muted">"Loading page…"</p> }>
                {move || page.get().map(|result| match result {
                    Ok(content) => view! { <PageBody content=content old=version.get().is_some()/> }.into_any(),
                    Err(e) => view! { <div class="empty"><h1>"Page not found"</h1><p>{e.message}</p></div> }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[component]
fn PageBody(content: WikiContent, old: bool) -> impl IntoView {
    let space = use_space();
    let key = space.key.get_untracked();
    let id = content.id.clone();
    let edit = format!("{}/edit", page_path(&key, &id));
    let history = format!("{}/history", page_path(&key, &id));
    let child = format!("/wiki/spaces/{key}/create?parent={id}");
    let html = content.view_html();
    let created_by = content.history.created_by.clone();
    let updated = content.version.clone();
    let labels = content.labels();
    view! {
        <nav class="breadcrumbs">
            <A href=format!("/wiki/spaces/{key}")>{content.space.name.clone()}</A>
            {content.ancestors.clone().into_iter().map(|a| view! {
                " / " <A href=page_path(&key, &a.id)>{a.title}</A>
            }).collect_view()}
        </nav>
        {old.then(|| view! { <OldVersionBanner id=id.clone() number=updated.number/> })}
        <header class="wiki-header">
            <h1 class="wiki-title">{content.title.clone()}</h1>
            <div class="issue-actions">
                <A href=edit attr:class="button primary">"Edit"</A>
                <A href=child attr:class="button">"Create child page"</A>
                <A href=history attr:class="button subtle">"History"</A>
                <DeletePage id=id.clone()/>
            </div>
            <p class="byline muted small">
                <Avatar user=Some(created_by.as_user()) size=20/>
                <span>{format!("Created by {}", created_by.display_name)}</span>
                <span>{format!("Last updated {} by {} (version {})", short_time(&updated.when), updated.by.display_name, updated.number)}</span>
            </p>
        </header>
        // The server sanitizes the view format, so it is safe to insert as HTML.
        <article class="wiki-body" inner_html=html></article>
        <Labels id=id.clone() labels=labels/>
        <Comments id=id/>
    }
}

#[component]
fn OldVersionBanner(id: String, number: i64) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let space = use_space();
    let error = RwSignal::new(None::<String>);
    let restore = move |_| {
        let id = id.clone();
        let navigate = navigate.clone();
        let key = space.key.get_untracked();
        spawn_local(async move {
            let body = json!({ "operationKey": "restore", "params": { "versionNumber": number, "message": format!("Restored version {number}"), "restoreTitle": true } });
            match api::post::<Value>(&format!("{}/content/{id}/version", api::WIKI), &body).await {
                Ok(_) => {
                    session.changed();
                    navigate(&page_path(&key, &id), Default::default());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="notice-bar">
            {format!("You are viewing version {number}, which is not the current version.")}
            <button class="button small" on:click=restore>"Restore this version"</button>
            <ErrorText error=error/>
        </div>
    }
}

#[component]
fn DeletePage(id: String) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let space = use_space();
    let confirming = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let id = StoredValue::new(id);
    let delete = move |_| {
        let navigate = navigate.clone();
        let key = space.key.get_untracked();
        spawn_local(async move {
            match api::delete(&format!("{}/pages/{}", api::WIKI2, id.get_value())).await {
                Ok(()) => {
                    session.changed();
                    navigate(&format!("/wiki/spaces/{key}"), Default::default());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Show when=move || confirming.get()
            fallback=move || view! { <button class="button subtle" on:click=move |_| confirming.set(true)>"Delete"</button> }>
            <span class="confirm">
                "Move this page to the trash?"
                <button class="button danger small" on:click=delete.clone()>"Delete"</button>
                <button class="button subtle small" on:click=move |_| confirming.set(false)>"Cancel"</button>
            </span>
        </Show>
        <ErrorText error=error/>
    }
}

#[component]
fn Labels(id: String, labels: Vec<String>) -> impl IntoView {
    let session = use_session();
    let draft = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let id = StoredValue::new(id);
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let names: Vec<Value> = draft
            .get()
            .split_whitespace()
            .map(|name| json!({ "prefix": "global", "name": name }))
            .collect();
        if names.is_empty() {
            return;
        }
        spawn_local(async move {
            match api::post::<Value>(
                &format!("{}/content/{}/label", api::WIKI, id.get_value()),
                &json!(names),
            )
            .await
            {
                Ok(_) => {
                    draft.set(String::new());
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let remove = move |name: String| {
        spawn_local(async move {
            let path = format!(
                "{}/content/{}/label/{}",
                api::WIKI,
                id.get_value(),
                api::encode(&name)
            );
            match api::delete(&path).await {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <section class="wiki-labels">
            {labels.into_iter().map(|name| {
                let label = name.clone();
                view! {
                    <span class="label-chip">
                        {name}
                        <button class="chip-remove" title="Remove label" on:click=move |_| remove(label.clone())>"×"</button>
                    </span>
                }
            }).collect_view()}
            <form class="label-form" on:submit=add>
                <input class="input small-input" placeholder="Add labels"
                    prop:value=move || draft.get()
                    on:input:target=move |ev| draft.set(ev.target().value())/>
            </form>
            <ErrorText error=error/>
        </section>
    }
}

#[component]
fn Comments(id: String) -> impl IntoView {
    let session = use_session();
    let id = StoredValue::new(id);
    let comments = LocalResource::new(move || {
        session.changes.track();
        async move {
            api::get::<WikiCollection<WikiContent>>(&format!(
                "{}/content/{}/child/comment?expand=body.view,history,version,ancestors&depth=all&limit=500",
                api::WIKI,
                id.get_value()
            ))
            .await
            .map(|c| c.results)
            .unwrap_or_default()
        }
    });
    view! {
        <section class="wiki-comments">
            <h2 class="section-title">{move || {
                let count = comments.get().map(|c| c.len()).unwrap_or(0);
                if count == 1 { "1 comment".to_string() } else { format!("{count} comments") }
            }}</h2>
            <CommentBox page=id.get_value() reply_to=None/>
            <ul class="comments">
                {move || {
                    let all = comments.get().unwrap_or_default();
                    // Replies follow the comment they answer.
                    threaded(all).into_iter().map(|(depth, comment)| view! {
                        <CommentItem comment=comment depth=depth page=id.get_value()/>
                    }).collect_view()
                }}
            </ul>
        </section>
    }
}

/// Orders comments so each reply follows its parent, with its depth.
fn threaded(all: Vec<WikiContent>) -> Vec<(usize, WikiContent)> {
    fn add(
        parent: Option<&str>,
        all: &[WikiContent],
        depth: usize,
        out: &mut Vec<(usize, WikiContent)>,
    ) {
        for comment in all
            .iter()
            .filter(|c| c.ancestors.last().map(|a| a.id.as_str()) == parent)
        {
            out.push((depth, comment.clone()));
            add(Some(&comment.id), all, depth + 1, out);
        }
    }
    let mut out = Vec::with_capacity(all.len());
    add(None, &all, 0, &mut out);
    out
}

#[component]
fn CommentItem(comment: WikiContent, depth: usize, page: String) -> impl IntoView {
    let session = use_session();
    let replying = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let author = comment.history.created_by.clone();
    let mine = author.account_id == session.me.get_value().account_id;
    let id = StoredValue::new(comment.id.clone());
    let delete = move |_| {
        spawn_local(async move {
            match api::delete(&format!(
                "{}/footer-comments/{}",
                api::WIKI2,
                id.get_value()
            ))
            .await
            {
                Ok(()) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let style = format!("margin-left: {}px", depth.min(6) * 32);
    let html = comment.view_html();
    view! {
        <li class="comment" style=style>
            <Avatar user=Some(author.as_user()) size=32/>
            <div class="grow">
                <strong>{author.display_name.clone()}</strong>
                <span class="muted small">{short_time(&comment.history.created_date)}</span>
                <div class="wiki-body comment-body" inner_html=html></div>
                <div class="comment-actions">
                    <button class="link-button small" on:click=move |_| replying.update(|r| *r = !*r)>"Reply"</button>
                    {mine.then(|| view! { <button class="link-button small" on:click=delete>"Delete"</button> })}
                </div>
                <ErrorText error=error/>
                <Show when=move || replying.get()>
                    <CommentBox page=page.clone() reply_to=Some(id.get_value())/>
                </Show>
            </div>
        </li>
    }
}

/// A box for a new comment, or a reply when `reply_to` is set. Markdown works.
#[component]
fn CommentBox(page: String, reply_to: Option<String>) -> impl IntoView {
    let session = use_session();
    let draft = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let target = StoredValue::new((page, reply_to));
    let save = move |_| {
        let text = draft.get();
        if text.trim().is_empty() {
            return;
        }
        let (page, reply_to) = target.get_value();
        let mut body = json!({ "body": adf_body(&text) });
        match reply_to {
            Some(parent) => body["parentCommentId"] = json!(parent),
            None => body["pageId"] = json!(page),
        }
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/footer-comments", api::WIKI2), &body).await {
                Ok(_) => {
                    draft.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="add-comment">
            <Avatar user=Some(session.me.get_value()) size=32/>
            <div class="grow">
                <textarea class="input" rows="3" placeholder="Add a comment… Markdown works."
                    prop:value=move || draft.get()
                    on:input:target=move |ev| draft.set(ev.target().value())></textarea>
                <ErrorText error=error/>
                <button class="button primary" on:click=save>"Save"</button>
            </div>
        </div>
    }
}

/// `/wiki/spaces/:key/pages/:id/history`: every version, newest first.
#[component]
pub fn PageHistory() -> impl IntoView {
    let params = use_params_map();
    let space = use_space();
    let id = Signal::derive(move || params.with(|p| p.get("id")).unwrap_or_default());
    let versions = LocalResource::new(move || {
        let id = id.get();
        async move {
            api::get::<WikiCollection<WikiVersion>>(&format!("{}/content/{id}/version", api::WIKI))
                .await
                .map(|c| c.results)
        }
    });
    view! {
        <div class="page wiki-page">
            <header class="page-header">
                <h1>"Page history"</h1>
                <A href=move || page_path(&space.key.get(), &id.get()) attr:class="button">"Back to page"</A>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading history…"</p> }>
                {move || versions.get().map(|result| match result {
                    Ok(list) => view! {
                        <table class="issue-table">
                            <thead><tr><th>"Version"</th><th>"Date"</th><th>"By"</th><th>"Comment"</th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|v| {
                                    let href = format!("{}?version={}", page_path(&space.key.get_untracked(), &id.get_untracked()), v.number);
                                    view! {
                                        <tr>
                                            <td><A href=href>{format!("v. {}", v.number)}</A></td>
                                            <td class="muted">{short_time(&v.when)}</td>
                                            <td><span class="person"><Avatar user=Some(v.by.as_user()) size=20/>{v.by.display_name.clone()}</span></td>
                                            <td class="muted">{v.message}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any(),
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
        </div>
    }
}
