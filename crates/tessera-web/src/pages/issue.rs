//! `/browse/:key`: the issue view.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

use super::{AgileFields, agile_fields};
use crate::api;
use crate::app::use_session;
use crate::components::{
    Avatar, ErrorText, Lozenge, TypeIcon, UserSelect, autofocus, render_adf, users_resource,
};
use crate::model::{Comment, Issue, IssueRef, LinkType, Named, Transition, User, short_time};

/// Saves fields of an issue, then tells open views to reload.
fn save_fields(key: String, fields: Value, error: RwSignal<Option<String>>) {
    let session = use_session();
    spawn_local(async move {
        match api::request(
            gloo_net::http::Method::PUT,
            &format!("{}/issue/{key}", api::CORE),
            Some(&json!({ "fields": fields })),
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
}

#[component]
pub fn IssuePage() -> impl IntoView {
    let params = use_params_map();
    let session = use_session();
    let key = Signal::derive(move || {
        params
            .with(|p| p.get("key"))
            .unwrap_or_default()
            .to_uppercase()
    });
    let issue = LocalResource::new(move || {
        let key = key.get();
        session.changes.track();
        async move { api::get::<Issue>(&format!("{}/issue/{key}", api::CORE)).await }
    });
    view! {
        <div class="page issue-page">
            <Transition fallback=|| view! { <p class="muted">"Loading issue…"</p> }>
                {move || issue.get().map(|result| match result {
                    Ok(issue) => view! { <IssueView issue=issue/> }.into_any(),
                    Err(e) => view! { <div class="empty"><h1>"Issue not found"</h1><p>{e.message}</p></div> }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[component]
fn IssueView(issue: Issue) -> impl IntoView {
    let key = issue.key.clone();
    let fields = issue.fields.clone();
    let error = RwSignal::new(None::<String>);
    let project = fields.project.clone().unwrap_or_default();
    view! {
        <nav class="breadcrumbs">
            <A href="/projects">"Projects"</A>
            " / "
            <A href=format!("/projects/{}", project.key)>{project.name.clone()}</A>
            {fields.parent.clone().map(|parent| view! {
                " / " <A href=format!("/browse/{}", parent.key)>{parent.key.clone()}</A>
            })}
            " / "
            <span class="issue-key"><TypeIcon item=fields.issuetype.clone()/>{key.clone()}</span>
        </nav>
        <div class="issue-layout">
            <div class="issue-main">
                <Summary key=key.clone() summary=fields.summary.clone() error=error/>
                <ErrorText error=error/>
                <div class="issue-actions">
                    <StatusButton key=key.clone() status=fields.status.clone()/>
                    <WatchButton key=key.clone()/>
                    <DeleteButton key=key.clone() project=project.key.clone()/>
                </div>
                <h3 class="section-title">"Description"</h3>
                <Description key=key.clone() description=fields.description.clone() error=error/>
                <ChildIssues issue=issue.clone()/>
                <Links issue=issue.clone()/>
                <Attachments issue=issue.clone()/>
                <Activity key=key.clone() comments=fields.comment.clone().map(|c| c.comments).unwrap_or_default()/>
            </div>
            <Details issue=issue error=error/>
        </div>
    }
}

/// The summary; click to edit, Enter to save.
#[component]
fn Summary(key: String, summary: String, error: RwSignal<Option<String>>) -> impl IntoView {
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(summary.clone());
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        editing.set(false);
        save_fields(key.clone(), json!({ "summary": draft.get() }), error);
    };
    view! {
        <Show when=move || editing.get()
            fallback=move || view! { <h1 class="issue-summary" title="Click to edit" on:click=move |_| editing.set(true)>{draft.get()}</h1> }>
            <form on:submit=save.clone()>
                <input class="input summary-input" node_ref=autofocus() prop:value=move || draft.get()
                    on:input:target=move |ev| draft.set(ev.target().value())/>
            </form>
        </Show>
    }
}

/// The current status; clicking shows the transitions available from it.
#[component]
fn StatusButton(key: String, status: Option<crate::model::Status>) -> impl IntoView {
    let session = use_session();
    let open = RwSignal::new(false);
    // A stored value is a copyable handle, so the menu can be rendered many times.
    let path = StoredValue::new(format!("{}/issue/{key}/transitions", api::CORE));
    let transitions = LocalResource::new(move || {
        let path = path.get_value();
        async move {
            let value: Value = api::get(&path).await.unwrap_or_default();
            serde_json::from_value::<Vec<Transition>>(value["transitions"].clone())
                .unwrap_or_default()
        }
    });
    let category = status
        .as_ref()
        .map(|s| s.status_category.key.clone())
        .unwrap_or_default();
    let name = status.map(|s| s.name).unwrap_or_default();
    view! {
        <div class="dropdown">
            <button class=format!("button status-button lozenge-{category}") on:click=move |_| open.update(|o| *o = !*o)>
                {name} " ▾"
            </button>
            <Show when=move || open.get()>
                <ul class="menu">
                    {move || transitions.get().unwrap_or_default().into_iter().map(|t| {
                        view! {
                            <li><button class="menu-item" on:click=move |_| {
                                open.set(false);
                                let path = path.get_value();
                                let body = json!({ "transition": { "id": t.id } });
                                spawn_local(async move {
                                    if api::request(gloo_net::http::Method::POST, &path, Some(&body)).await.is_ok() {
                                        session.changed();
                                    }
                                });
                            }>{t.name.clone()} <span class="muted">" → "</span> <Lozenge status=Some(t.to.clone())/></button></li>
                        }
                    }).collect_view()}
                </ul>
            </Show>
        </div>
    }
}

#[component]
fn WatchButton(key: String) -> impl IntoView {
    let session = use_session();
    let path = format!("{}/issue/{key}/watchers", api::CORE);
    let read_path = path.clone();
    let watchers = LocalResource::new(move || {
        let path = read_path.clone();
        session.changes.track();
        async move { api::get::<Value>(&path).await.unwrap_or_default() }
    });
    let toggle = move |_| {
        let watching = watchers
            .get_untracked()
            .map(|w| w["isWatching"] == json!(true))
            .unwrap_or(false);
        let path = path.clone();
        let me = session.me.get_value().account_id;
        spawn_local(async move {
            let result = if watching {
                api::delete(&format!("{path}?accountId={me}")).await
            } else {
                api::request(gloo_net::http::Method::POST, &path, None)
                    .await
                    .map(|_| ())
            };
            if result.is_ok() {
                session.changed();
            }
        });
    };
    view! {
        <button class="button" on:click=toggle>
            {move || {
                let w = watchers.get().unwrap_or_default();
                let count = w["watchCount"].as_i64().unwrap_or(0);
                if w["isWatching"] == json!(true) { format!("Watching ({count})") } else { format!("Watch ({count})") }
            }}
        </button>
    }
}

#[component]
fn DeleteButton(key: String, project: String) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let confirming = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let delete = move |_| {
        let key = key.clone();
        let project = project.clone();
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::delete(&format!("{}/issue/{key}?deleteSubtasks=true", api::CORE)).await {
                Ok(()) => {
                    session.changed();
                    navigate(&format!("/projects/{project}/issues"), Default::default());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Show when=move || confirming.get()
            fallback=move || view! { <button class="button subtle" on:click=move |_| confirming.set(true)>"Delete"</button> }>
            <span class="confirm">
                "Delete this issue and its subtasks? "
                <button class="button danger" on:click=delete.clone()>"Delete"</button>
                <button class="button subtle" on:click=move |_| confirming.set(false)>"Cancel"</button>
            </span>
        </Show>
        <ErrorText error=error/>
    }
}

/// The description, rendered; editing uses Markdown.
#[component]
fn Description(key: String, description: Value, error: RwSignal<Option<String>>) -> impl IntoView {
    let editing = RwSignal::new(false);
    let markdown = serde_json::from_value::<tessera_adf::Node>(description.clone())
        .map(|doc| tessera_adf::to_markdown(&doc))
        .unwrap_or_default();
    let draft = RwSignal::new(markdown);
    let save = move |_| {
        editing.set(false);
        let text = draft.get();
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::to_value(tessera_adf::from_markdown(&text)).unwrap_or(Value::Null)
        };
        save_fields(key.clone(), json!({ "description": value }), error);
    };
    let shown = StoredValue::new(description);
    view! {
        <Show when=move || editing.get()
            fallback=move || view! { <div class="editable" title="Click to edit" on:click=move |_| editing.set(true)>{render_adf(&shown.get_value())}</div> }>
            <textarea class="input editor" rows="10" node_ref=autofocus() placeholder="Describe the work. Markdown works: **bold**, *italic*, lists, `code`."
                prop:value=move || draft.get()
                on:input:target=move |ev| draft.set(ev.target().value())></textarea>
            <div class="form-actions">
                <button class="button primary" on:click=save.clone()>"Save"</button>
                <button class="button subtle" on:click=move |_| editing.set(false)>"Cancel"</button>
            </div>
        </Show>
    }
}

fn issue_ref_row(issue: IssueRef) -> impl IntoView {
    issue_ref_row_with(issue, ())
}

/// A row for a related issue, with `extra` at the end, such as a remove button.
fn issue_ref_row_with(issue: IssueRef, extra: impl IntoView) -> impl IntoView {
    let href = format!("/browse/{}", issue.key);
    view! {
        <li class="ref-row">
            <TypeIcon item=issue.fields.issuetype/>
            <A href=href.clone() attr:class="issue-key">{issue.key}</A>
            <A href=href attr:class="summary-link grow">{issue.fields.summary}</A>
            <Lozenge status=issue.fields.status/>
            {extra}
        </li>
    }
}

/// Subtasks, and for epics the issues in the epic.
#[component]
fn ChildIssues(issue: Issue) -> impl IntoView {
    let session = use_session();
    let is_epic = issue
        .fields
        .issuetype
        .as_ref()
        .is_some_and(|t| t.name == "Epic");
    let key = issue.key.clone();
    let epic_children = LocalResource::new(move || {
        let key = key.clone();
        session.changes.track();
        async move {
            if !is_epic {
                return Vec::new();
            }
            super::search(&format!("parent = {key} ORDER BY Rank ASC"), 0, 200)
                .await
                .map(|page| {
                    page.issues
                        .into_iter()
                        .map(|i| IssueRef {
                            id: i.id,
                            key: i.key,
                            fields: crate::model::RefFields {
                                summary: i.fields.summary,
                                status: i.fields.status,
                                issuetype: i.fields.issuetype,
                                priority: i.fields.priority,
                            },
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    });
    let subtasks = issue.fields.subtasks.clone();
    let can_have_subtasks = !is_epic && !issue.fields.issuetype.as_ref().is_some_and(|t| t.subtask);
    let adding = RwSignal::new(false);
    let summary = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let project_key = StoredValue::new(
        issue
            .fields
            .project
            .clone()
            .map(|p| p.key)
            .unwrap_or_default(),
    );
    let parent_key = StoredValue::new(issue.key.clone());
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "fields": {
            "project": { "key": project_key.get_value() },
            "parent": { "key": parent_key.get_value() },
            "issuetype": { "name": if is_epic { "Story" } else { "Subtask" } },
            "summary": summary.get(),
        }});
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/issue", api::CORE), &body).await {
                Ok(_) => {
                    summary.set(String::new());
                    adding.set(false);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let title = if is_epic {
        "Issues in this epic"
    } else {
        "Subtasks"
    };
    view! {
        <Show when=move || is_epic || can_have_subtasks>
            <h3 class="section-title">{title}
                <button class="button subtle small" on:click=move |_| adding.set(true)>"+ Add"</button>
            </h3>
            <ul class="ref-list">
                {subtasks.clone().into_iter().map(issue_ref_row).collect_view()}
                {move || epic_children.get().unwrap_or_default().into_iter().map(issue_ref_row).collect_view()}
            </ul>
            <Show when=move || adding.get()>
                <form class="inline-form" on:submit=add>
                    <input class="input" placeholder="What needs to be done?" required node_ref=autofocus()
                        prop:value=move || summary.get()
                        on:input:target=move |ev| summary.set(ev.target().value())/>
                    <button class="button primary" type="submit">"Create"</button>
                </form>
                <ErrorText error=error/>
            </Show>
        </Show>
    }
}

#[component]
fn Links(issue: Issue) -> impl IntoView {
    let session = use_session();
    let key = StoredValue::new(issue.key.clone());
    let adding = RwSignal::new(false);
    // The chosen phrase, as "type name|outward" or "type name|inward".
    let choice = RwSignal::new(String::new());
    let other = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let link_types = LocalResource::new(|| async {
        let value: Value = api::get(&format!("{}/issueLinkType", api::CORE))
            .await
            .unwrap_or_default();
        serde_json::from_value::<Vec<LinkType>>(value["issueLinkTypes"].clone()).unwrap_or_default()
    });
    // Each link type reads two ways, as in Jira's "Link issue" menu.
    let phrases = move || {
        link_types
            .get()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|t| {
                [
                    (t.outward.clone(), format!("{}|outward", t.name)),
                    (t.inward.clone(), format!("{}|inward", t.name)),
                ]
            })
            .collect::<Vec<_>>()
    };
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let selected = choice.get();
        let selected = if selected.is_empty() {
            phrases().first().map(|p| p.1.clone()).unwrap_or_default()
        } else {
            selected
        };
        let Some((type_name, side)) = selected.rsplit_once('|') else {
            return;
        };
        let this = json!({ "key": key.get_value() });
        let that = json!({ "key": other.get().trim().to_uppercase() });
        // "This <outward phrase> that" makes this issue the outward issue.
        let (outward, inward) = if side == "outward" {
            (this, that)
        } else {
            (that, this)
        };
        let body = json!({ "type": { "name": type_name }, "outwardIssue": outward, "inwardIssue": inward });
        spawn_local(async move {
            match api::request(
                gloo_net::http::Method::POST,
                &format!("{}/issueLink", api::CORE),
                Some(&body),
            )
            .await
            {
                Ok(_) => {
                    adding.set(false);
                    other.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let remove = move |id: String| {
        spawn_local(async move {
            match api::delete(&format!("{}/issueLink/{id}", api::CORE)).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let links = issue.fields.issuelinks;
    view! {
        <h3 class="section-title">
            "Linked issues"
            <button class="button subtle small" on:click=move |_| adding.set(true)>"+ Add"</button>
        </h3>
        <ul class="ref-list">
            {links.into_iter().filter_map(|link| {
                // Outward: "blocks KEY"; inward: "is blocked by KEY".
                let (label, other) = match (link.outward_issue, link.inward_issue) {
                    (Some(other), _) => (link.link_type.outward, other),
                    (None, Some(other)) => (link.link_type.inward, other),
                    _ => return None,
                };
                let id = link.id;
                Some(view! {
                    <li class="link-label muted small">{label}</li>
                    {issue_ref_row_with(other, view! {
                        <button class="button subtle small" title="Remove link" on:click=move |_| remove(id.clone())>"Remove"</button>
                    })}
                })
            }).collect_view()}
        </ul>
        <Show when=move || adding.get()>
            <form class="inline-form" on:submit=add>
                <select class="select link-type" on:change:target=move |ev| choice.set(ev.target().value())>
                    {move || phrases().into_iter().map(|(label, value)| view! { <option value=value>{label}</option> }).collect_view()}
                </select>
                <input class="input" placeholder="Issue key, such as PROJ-1" required node_ref=autofocus()
                    prop:value=move || other.get()
                    on:input:target=move |ev| other.set(ev.target().value())/>
                <button class="button primary" type="submit">"Link"</button>
                <button class="button subtle" type="button" on:click=move |_| adding.set(false)>"Cancel"</button>
            </form>
        </Show>
        <ErrorText error=error/>
    }
}

#[component]
fn Attachments(issue: Issue) -> impl IntoView {
    let session = use_session();
    let key = issue.key.clone();
    let error = RwSignal::new(None::<String>);
    let upload = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        let Some(files) = input.files() else { return };
        let Ok(form) = web_sys::FormData::new() else {
            return;
        };
        for i in 0..files.length() {
            if let Some(file) = files.get(i) {
                let _ = form.append_with_blob_and_filename("file", &file, &file.name());
            }
        }
        let path = format!("{}/issue/{key}/attachments", api::CORE);
        spawn_local(async move {
            let request = gloo_net::http::Request::post(&path)
                .header("X-Atlassian-Token", "no-check")
                .body(form);
            let result = match request {
                Ok(request) => request.send().await.map(|r| r.ok()),
                Err(e) => Err(e),
            };
            match result {
                Ok(true) => session.changed(),
                _ => error.set(Some("The upload failed.".into())),
            }
        });
    };
    let attachments = issue.fields.attachment;
    view! {
        <h3 class="section-title">"Attachments"
            <label class="button subtle small">"+ Add"
                <input type="file" multiple class="visually-hidden" on:change=upload/>
            </label>
        </h3>
        <ErrorText error=error/>
        <ul class="attachments">
            {attachments.into_iter().map(|a| {
                let size = if a.size >= 1024 * 1024 { format!("{:.1} MB", a.size as f64 / 1048576.0) } else { format!("{} KB", (a.size + 1023) / 1024) };
                view! {
                    <li class="attachment">
                        <a href=a.content target="_blank" rel="noopener">{a.filename}</a>
                        <span class="muted small">{size} " · " {short_time(&a.created)}</span>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}

/// Comments and history, in tabs.
#[component]
fn Activity(key: String, comments: Vec<Comment>) -> impl IntoView {
    let tab = RwSignal::new("comments");
    let history_key = key.clone();
    let history = LocalResource::new(move || {
        let key = history_key.clone();
        async move {
            api::get::<Value>(&format!(
                "{}/issue/{key}/changelog?maxResults=100",
                api::CORE
            ))
            .await
            .unwrap_or_default()
        }
    });
    view! {
        <h3 class="section-title">"Activity"</h3>
        <div class="tabs">
            <button class="tab" class:active=move || tab.get() == "comments" on:click=move |_| tab.set("comments")>"Comments"</button>
            <button class="tab" class:active=move || tab.get() == "history" on:click=move |_| tab.set("history")>"History"</button>
        </div>
        <Show when=move || tab.get() == "comments"
            fallback=move || view! {
                <ul class="history">
                    {move || history.get().map(|h| h["values"].as_array().cloned().unwrap_or_default().into_iter().rev().map(|entry| {
                        let author: Option<User> = serde_json::from_value(entry["author"].clone()).ok();
                        let name = author.as_ref().map(|a| a.display_name.clone()).unwrap_or_default();
                        let when = short_time(entry["created"].as_str().unwrap_or_default());
                        let items = entry["items"].as_array().cloned().unwrap_or_default();
                        view! {
                            <li class="history-entry">
                                <Avatar user=author size=24/>
                                <div>
                                    <strong>{name}</strong> <span class="muted small">{when}</span>
                                    {items.into_iter().map(|item| {
                                        let field = item["field"].as_str().unwrap_or_default().to_string();
                                        let from = item["fromString"].as_str().unwrap_or("None").to_string();
                                        let to = item["toString"].as_str().unwrap_or("None").to_string();
                                        view! { <p class="small">"changed " <strong>{field}</strong> ": " <s class="muted">{from}</s> " → " {to}</p> }
                                    }).collect_view()}
                                </div>
                            </li>
                        }
                    }).collect_view())}
                </ul>
            }>
            <AddComment key=key.clone()/>
            <ul class="comments">
                {comments.clone().into_iter().rev().map(|comment| view! { <CommentItem key=key.clone() comment=comment/> }).collect_view()}
            </ul>
        </Show>
    }
}

#[component]
fn AddComment(key: String) -> impl IntoView {
    let session = use_session();
    let draft = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = draft.get();
        if text.trim().is_empty() {
            return;
        }
        let body = json!({ "body": tessera_adf::from_markdown(&text) });
        let key = key.clone();
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/issue/{key}/comment", api::CORE), &body).await {
                Ok(_) => {
                    draft.set(String::new());
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <form class="add-comment" on:submit=submit>
            <Avatar user=Some(session.me.get_value()) size=32/>
            <div class="grow">
                <textarea class="input" rows="3" placeholder="Add a comment…"
                    prop:value=move || draft.get()
                    on:input:target=move |ev| draft.set(ev.target().value())></textarea>
                <ErrorText error=error/>
                <button class="button primary" type="submit">"Save"</button>
            </div>
        </form>
    }
}

#[component]
fn CommentItem(key: String, comment: Comment) -> impl IntoView {
    let session = use_session();
    let mine = comment
        .author
        .as_ref()
        .is_some_and(|a| a.account_id == session.me.get_value().account_id);
    let id = comment.id.clone();
    let delete = move |_| {
        let path = format!("{}/issue/{key}/comment/{id}", api::CORE);
        spawn_local(async move {
            if api::delete(&path).await.is_ok() {
                session.changed();
            }
        });
    };
    let name = comment
        .author
        .as_ref()
        .map(|a| a.display_name.clone())
        .unwrap_or_default();
    view! {
        <li class="comment">
            <Avatar user=comment.author.clone() size=32/>
            <div class="grow">
                <strong>{name}</strong> <span class="muted small">{short_time(&comment.created)}</span>
                {render_adf(&comment.body)}
                <Show when=move || mine>
                    <button class="button subtle small" on:click=delete.clone()>"Delete"</button>
                </Show>
            </div>
        </li>
    }
}

/// The details panel: people, priority, dates, labels, and Agile fields.
#[component]
fn Details(issue: Issue, error: RwSignal<Option<String>>) -> impl IntoView {
    let users = users_resource();
    let agile = agile_fields();
    let fields = issue.fields.clone();
    let key = issue.key.clone();
    let priorities = LocalResource::new(|| async {
        api::get::<Vec<Named>>(&format!("{}/priority", api::CORE))
            .await
            .unwrap_or_default()
    });

    let set = {
        let key = key.clone();
        move |name: &str, value: Value| {
            let fields = serde_json::Map::from_iter([(name.to_string(), value)]);
            save_fields(key.clone(), Value::Object(fields), error)
        }
    };
    let set_assignee = {
        let set = set.clone();
        Callback::new(move |id: Option<String>| {
            set(
                "assignee",
                id.map(|id| json!({ "accountId": id }))
                    .unwrap_or(Value::Null),
            )
        })
    };
    let set_reporter = {
        let set = set.clone();
        Callback::new(move |id: Option<String>| {
            set(
                "reporter",
                id.map(|id| json!({ "accountId": id }))
                    .unwrap_or(Value::Null),
            )
        })
    };
    // The levels this user may pick, from the edit metadata; none when they may not set one.
    let levels = LocalResource::new({
        let key = key.clone();
        move || {
            let key = key.clone();
            async move {
                api::get::<Value>(&format!("{}/issue/{key}/editmeta", api::CORE))
                    .await
                    .ok()
                    .and_then(|meta| {
                        meta["fields"]["security"]["allowedValues"]
                            .as_array()
                            .cloned()
                    })
            }
        }
    });
    let current_level = fields.security.clone();
    let set_level = set.clone();
    let current_priority = fields.priority.as_ref().map(|p| p.id.clone());
    let labels = RwSignal::new(fields.labels.join(" "));
    let due = fields.duedate.clone().unwrap_or_default();
    let set_priority = set.clone();
    let set_labels = set.clone();
    let set_due = set.clone();
    let set_points = set.clone();
    let story_points = move |agile: &AgileFields| {
        agile
            .story_points
            .as_ref()
            .and_then(|id| fields.extra.get(id))
            .and_then(Value::as_f64)
            .map(|v| v.to_string())
            .unwrap_or_default()
    };
    let sprint_names = {
        let extra = issue.fields.extra.clone();
        move |agile: &AgileFields| {
            agile
                .sprint
                .as_ref()
                .and_then(|id| extra.get(id))
                .and_then(Value::as_array)
                .map(|sprints| {
                    sprints
                        .iter()
                        .filter_map(|s| s["name"].as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        }
    };
    let assignee_id = fields.assignee.as_ref().map(|u| u.account_id.clone());
    let reporter_id = fields.reporter.as_ref().map(|u| u.account_id.clone());
    let me = use_session().me.get_value().account_id;
    let assign_me = set_assignee;
    view! {
        <aside class="details">
            <h3 class="section-title">"Details"</h3>
            <dl class="field-list">
                <dt>"Assignee"</dt>
                <dd>
                    <UserSelect users=users selected=assignee_id on_change=set_assignee/>
                    <button class="link-button small" on:click=move |_| assign_me.run(Some(me.clone()))>"Assign to me"</button>
                </dd>
                <dt>"Reporter"</dt>
                <dd><UserSelect users=users selected=reporter_id on_change=set_reporter/></dd>
                <dt>"Priority"</dt>
                <dd>
                    <select class="select" on:change:target=move |ev| set_priority("priority", json!({ "id": ev.target().value() }))>
                        {move || priorities.get().unwrap_or_default().into_iter().map(|p| {
                            let selected = current_priority.as_deref() == Some(p.id.as_str());
                            view! { <option value=p.id selected=selected>{p.name}</option> }
                        }).collect_view()}
                    </select>
                </dd>
                {move || {
                    let current = current_level.clone();
                    let set_level = set_level.clone();
                    match (levels.get().flatten(), current) {
                        (Some(allowed), current) => Some(view! {
                            <dt>"Security level"</dt>
                            <dd>
                                <select class="select" aria-label="Security level"
                                    on:change:target=move |ev| {
                                        let id = ev.target().value();
                                        set_level("security", if id.is_empty() { Value::Null } else { json!({ "id": id }) });
                                    }>
                                    <option value="" selected=current.is_none()>"None"</option>
                                    {allowed.into_iter().map(|level| {
                                        let id = level["id"].as_str().unwrap_or_default().to_string();
                                        let selected = current.as_ref().is_some_and(|c| c.id == id);
                                        view! { <option value=id selected=selected>{level["name"].as_str().unwrap_or_default().to_string()}</option> }
                                    }).collect_view()}
                                </select>
                            </dd>
                        }.into_any()),
                        (None, Some(level)) => Some(view! {
                            <dt>"Security level"</dt>
                            <dd class="security-level" title="Only members of this level can see the issue">
                                "🔒 "{level.name}
                            </dd>
                        }.into_any()),
                        (None, None) => None,
                    }
                }}
                <dt>"Labels"</dt>
                <dd>
                    <input class="input" placeholder="Separate with spaces"
                        prop:value=move || labels.get()
                        on:input:target=move |ev| labels.set(ev.target().value())
                        on:change=move |_| {
                            let list: Vec<String> = labels.get().split_whitespace().map(str::to_string).collect();
                            set_labels("labels", json!(list));
                        }/>
                </dd>
                <dt>"Due date"</dt>
                <dd>
                    <input class="input" type="date" prop:value=due
                        on:change:target=move |ev| {
                            let value = ev.target().value();
                            set_due("duedate", if value.is_empty() { Value::Null } else { json!(value) });
                        }/>
                </dd>
                {move || agile.get().map(|agile| {
                    let set_points = set_points.clone();
                    let points_field = agile.story_points.clone();
                    let points = story_points(&agile);
                    let sprints = sprint_names(&agile);
                    view! {
                        <dt>"Story points"</dt>
                        <dd>
                            <input class="input" type="number" min="0" step="0.5" prop:value=points
                                on:change:target=move |ev| {
                                    if let Some(field) = points_field.clone() {
                                        let value = ev.target().value();
                                        set_points(&field, value.parse::<f64>().map(|v| json!(v)).unwrap_or(Value::Null));
                                    }
                                }/>
                        </dd>
                        <dt>"Sprint"</dt>
                        <dd>{if sprints.is_empty() { "None".to_string() } else { sprints }}</dd>
                    }
                })}
                <dt>"Components"</dt>
                <dd>{list_names(&issue.fields.components)}</dd>
                <dt>"Fix versions"</dt>
                <dd>{list_names(&issue.fields.fix_versions)}</dd>
                {issue.fields.parent.clone().map(|parent| view! {
                    <dt>"Parent"</dt>
                    <dd><A href=format!("/browse/{}", parent.key) attr:class="issue-key">{parent.key.clone()}</A> " " {parent.fields.summary.clone()}</dd>
                })}
            </dl>
            <p class="muted small">"Created " {short_time(&issue.fields.created)}</p>
            <p class="muted small">"Updated " {short_time(&issue.fields.updated)}</p>
        </aside>
    }
}

fn list_names(items: &[Named]) -> String {
    if items.is_empty() {
        "None".into()
    } else {
        items
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
