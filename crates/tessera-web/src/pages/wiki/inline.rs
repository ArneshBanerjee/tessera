//! Inline comments: select text on a page to comment on it. Commented text is highlighted;
//! the comments show beside the page, where they can be answered, resolved, and reopened.

use std::collections::HashMap;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::adf_body;
use crate::api;
use crate::app::use_session;
use crate::components::{Avatar, ErrorText, MarkdownArea, users_resource};
use crate::model::{User, short_time};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Version {
    #[serde(rename = "authorId")]
    author_id: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Properties {
    #[serde(rename = "inlineMarkerRef")]
    marker: String,
    #[serde(rename = "inlineOriginalSelection")]
    selection: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct InlineComment {
    id: String,
    version: Version,
    body: Value,
    #[serde(rename = "resolutionStatus")]
    resolution: String,
    properties: Properties,
}

impl InlineComment {
    fn html(&self) -> String {
        self.body["view"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Page<T> {
    results: Vec<T>,
}

/// A thread: a comment and its replies.
type Thread = (InlineComment, Vec<InlineComment>);

/// Text the user selected, how many times it occurs before the selection, and where to show
/// the Comment button, relative to the page body.
#[derive(Clone, Debug, PartialEq)]
struct Selected {
    text: String,
    index: usize,
    top: f64,
    left: f64,
}

/// Reads the selection inside `article`, if there is one.
fn selected_in(article: &web_sys::Element) -> Option<Selected> {
    let window = web_sys::window()?;
    let selection = window.get_selection().ok()??;
    if selection.is_collapsed() || selection.range_count() == 0 {
        return None;
    }
    let text: String = selection.to_string().into();
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    let ancestor = range.common_ancestor_container().ok()?;
    if !article.contains(Some(&ancestor)) {
        return None;
    }
    // The page text before the selection decides which occurrence was picked.
    let before = window.document()?.create_range().ok()?;
    before.set_start(article, 0).ok()?;
    before
        .set_end(&range.start_container().ok()?, range.start_offset().ok()?)
        .ok()?;
    let prefix: String = before.to_string().into();
    let index = prefix.matches(text.as_str()).count();
    let rect = range.get_bounding_client_rect();
    let frame = article.get_bounding_client_rect();
    Some(Selected {
        text,
        index,
        top: rect.top() - frame.top() - 36.0,
        left: (rect.left() - frame.left()).max(0.0),
    })
}

/// Highlights the markers of one comment, and clears the others.
fn highlight(article: &web_sys::Element, marker: Option<&str>) {
    let Ok(spans) = article.query_selector_all("[data-ref]") else {
        return;
    };
    for i in 0..spans.length() {
        let Some(span) = spans
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let on = marker.is_some_and(|m| span.get_attribute("data-ref").as_deref() == Some(m));
        let _ = if on {
            span.class_list().add_1("active")
        } else {
            span.class_list().remove_1("active")
        };
    }
}

/// The page body, with the inline comments on it. `html` is the sanitized view of the page.
#[component]
pub fn CommentedBody(page_id: String, kind: String, html: String, current: bool) -> impl IntoView {
    let session = use_session();
    let article = NodeRef::<html::Article>::new();
    let users = users_resource();
    let selected = RwSignal::new(None::<Selected>);
    let composing = RwSignal::new(None::<Selected>);
    let focused = RwSignal::new(None::<String>);
    let show_resolved = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let collection = if kind == "blogpost" {
        "blogposts"
    } else {
        "pages"
    };
    let page = StoredValue::new((page_id, collection));
    let threads = LocalResource::new(move || {
        session.changes.track();
        let (id, collection) = page.get_value();
        async move {
            let top: Page<InlineComment> = api::get(&format!(
                "{}/{collection}/{id}/inline-comments?body-format=view&limit=250",
                api::WIKI2
            ))
            .await
            .unwrap_or_default();
            let mut out: Vec<Thread> = Vec::new();
            for comment in top.results {
                let replies: Page<InlineComment> = api::get(&format!(
                    "{}/inline-comments/{}/children?body-format=view&limit=250",
                    api::WIKI2,
                    comment.id
                ))
                .await
                .unwrap_or_default();
                out.push((comment, replies.results));
            }
            out
        }
    });
    let people = move || -> HashMap<String, User> {
        users
            .get()
            .unwrap_or_default()
            .into_iter()
            .map(|u| (u.account_id.clone(), u))
            .collect()
    };
    // Text whose comments are resolved is no longer highlighted, as in Confluence.
    Effect::new(move |_| {
        let resolved: Vec<String> = threads
            .get()
            .unwrap_or_default()
            .iter()
            .filter(|(c, _)| c.resolution == "resolved")
            .map(|(c, _)| c.properties.marker.clone())
            .collect();
        let Some(element) = article.get() else {
            return;
        };
        let Ok(spans) = element.query_selector_all("[data-ref]") else {
            return;
        };
        for i in 0..spans.length() {
            if let Some(span) = spans.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) {
                let done = span.get_attribute("data-ref").is_some_and(|r| resolved.contains(&r));
                let _ = span.class_list().toggle_with_force("resolved", done);
            }
        }
    });
    // Marks follow the comment in focus.
    Effect::new(move |_| {
        let marker = focused.get();
        if let Some(element) = article.get() {
            highlight(&element, marker.as_deref());
        }
    });
    let on_mouseup = move |_| {
        if !current {
            return;
        }
        if let Some(element) = article.get_untracked() {
            selected.set(selected_in(&element));
        }
    };
    let on_click = move |ev: leptos::ev::MouseEvent| {
        let marker = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|e| e.closest("[data-ref]").ok().flatten())
            .and_then(|e| e.get_attribute("data-ref"));
        if let Some(marker) = marker {
            focused.set(Some(marker.clone()));
            if let Some(card) = web_sys::window().and_then(|w| w.document()).and_then(|d| {
                d.query_selector(&format!("[data-thread=\"{marker}\"]"))
                    .ok()
                    .flatten()
            }) {
                card.scroll_into_view();
            }
        }
    };
    let draft = RwSignal::new(String::new());
    let save = move |_| {
        let Some(chosen) = composing.get_untracked() else {
            return;
        };
        let (id, collection) = page.get_value();
        let key = if collection == "blogposts" {
            "blogPostId"
        } else {
            "pageId"
        };
        let mut body = json!({
            "body": adf_body(&draft.get_untracked()),
            "inlineCommentProperties": {
                "textSelection": chosen.text,
                "textSelectionMatchCount": 1,
                "textSelectionMatchIndex": chosen.index,
            },
        });
        body[key] = json!(id);
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/inline-comments", api::WIKI2), &body).await {
                Ok(_) => {
                    composing.set(None);
                    draft.set(String::new());
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        // The panel takes room only when there are comments to show.
        <div class="commented-body"
            class:with-panel=move || composing.get().is_some() || threads.get().is_some_and(|t| !t.is_empty())>
            <div class="commented-text">
                // The server sanitizes the view format, so it is safe to insert as HTML.
                <article class="wiki-body" node_ref=article inner_html=html on:mouseup=on_mouseup on:click=on_click></article>
                {move || selected.get().map(|chosen| {
                    let style = format!("top: {}px; left: {}px", chosen.top, chosen.left);
                    view! {
                        <button class="button small inline-comment-button" style=style
                            on:mousedown=move |ev| ev.prevent_default()
                            on:click=move |_| { composing.set(Some(chosen.clone())); selected.set(None); }>
                            "💬 Comment"
                        </button>
                    }
                })}
            </div>
            <aside class="inline-panel" aria-label="Inline comments">
                {move || composing.get().map(|chosen| view! {
                    <div class="inline-thread composing">
                        <blockquote class="inline-quote">{chosen.text.clone()}</blockquote>
                        <MarkdownArea value=draft rows=3 placeholder="Comment on this text… Type @ to mention someone." autofocus=true/>
                        <ErrorText error=error/>
                        <div class="form-actions">
                            <button class="button primary small" on:click=save>"Comment"</button>
                            <button class="button subtle small" on:click=move |_| composing.set(None)>"Cancel"</button>
                        </div>
                    </div>
                })}
                {move || {
                    let all = threads.get().unwrap_or_default();
                    let resolved = all.iter().filter(|(c, _)| c.resolution == "resolved").count();
                    let shown: Vec<Thread> = all
                        .into_iter()
                        .filter(|(c, _)| show_resolved.get() || c.resolution != "resolved")
                        .collect();
                    let people = people();
                    view! {
                        {shown.into_iter().map(|thread| view! {
                            <ThreadCard thread=thread people=people.clone() focused=focused/>
                        }).collect_view()}
                        {(resolved > 0).then(|| view! {
                            <button class="link-button small" on:click=move |_| show_resolved.update(|s| *s = !*s)>
                                {move || if show_resolved.get() { "Hide resolved comments".to_string() } else { format!("Show {resolved} resolved") }}
                            </button>
                        })}
                    }
                }}
            </aside>
        </div>
    }
}

#[component]
fn ThreadCard(
    thread: Thread,
    people: HashMap<String, User>,
    focused: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = use_session();
    let (comment, replies) = thread;
    let marker = comment.properties.marker.clone();
    let id = StoredValue::new(comment.id.clone());
    let reply = RwSignal::new(String::new());
    let replying = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let resolved = comment.resolution == "resolved";
    let set_resolved = move |value: bool| {
        spawn_local(async move {
            let path = format!("{}/inline-comments/{}", api::WIKI2, id.get_value());
            match api::put::<Value>(&path, &json!({ "resolved": value })).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let send_reply = move |_| {
        let body =
            json!({ "parentCommentId": id.get_value(), "body": adf_body(&reply.get_untracked()) });
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/inline-comments", api::WIKI2), &body).await {
                Ok(_) => {
                    reply.set(String::new());
                    replying.set(false);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let status_note = match comment.resolution.as_str() {
        "dangling" => Some("The text this comment was on has changed."),
        "resolved" => Some("Resolved"),
        _ => None,
    };
    let marker_for_focus = marker.clone();
    let entry = move |c: &InlineComment| {
        let author = people.get(&c.version.author_id).cloned();
        let name = author
            .as_ref()
            .map_or_else(|| "Someone".to_string(), |u| u.display_name.clone());
        let html = c.html();
        view! {
            <div class="inline-entry">
                <div class="inline-author">
                    <Avatar user=author size=20/>
                    <strong>{name}</strong>
                    <span class="muted small">{short_time(&c.version.created_at)}</span>
                </div>
                // Sanitized by the server.
                <div class="wiki-body comment-body" inner_html=html></div>
            </div>
        }
    };
    view! {
        <div class="inline-thread" class:focused=move || focused.get().as_deref() == Some(marker.as_str())
            class:resolved=resolved attr:data-thread=marker_for_focus.clone()
            on:click=move |_| focused.set(Some(marker_for_focus.clone()))>
            <blockquote class="inline-quote">{comment.properties.selection.clone()}</blockquote>
            {status_note.map(|note| view! { <p class="muted small">{note}</p> })}
            {entry(&comment)}
            {replies.iter().map(&entry).collect_view()}
            <ErrorText error=error/>
            <Show when=move || replying.get()
                fallback=move || view! {
                    <div class="inline-actions">
                        <button class="link-button small" on:click=move |_| replying.set(true)>"Reply"</button>
                        {if resolved {
                            view! { <button class="link-button small" on:click=move |_| set_resolved(false)>"Reopen"</button> }.into_any()
                        } else {
                            view! { <button class="link-button small" on:click=move |_| set_resolved(true)>"Resolve"</button> }.into_any()
                        }}
                    </div>
                }>
                <MarkdownArea value=reply rows=2 placeholder="Reply…" autofocus=true/>
                <div class="form-actions">
                    <button class="button primary small" on:click=send_reply>"Reply"</button>
                    <button class="button subtle small" on:click=move |_| replying.set(false)>"Cancel"</button>
                </div>
            </Show>
        </div>
    }
}
