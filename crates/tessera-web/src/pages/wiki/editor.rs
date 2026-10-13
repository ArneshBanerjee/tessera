//! The page editor, for new pages and edits. Pages are written in Markdown and saved as ADF;
//! the server keeps them in Confluence's storage format.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use serde_json::{Value, json};

use super::{adf_body, page_path, use_space};
use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, autofocus};

/// `/wiki/spaces/:key/create?parent=ID`: a new page under `parent`, or under the home page.
#[component]
pub fn CreatePage() -> impl IntoView {
    let query = use_query_map();
    let parent = query.with_untracked(|q| q.get("parent"));
    view! { <Editor id=None parent=parent/> }
}

/// `/wiki/spaces/:key/pages/:id/edit`.
#[component]
pub fn EditPage() -> impl IntoView {
    let params = use_params_map();
    let id = params.with_untracked(|p| p.get("id"));
    view! { <Editor id=id parent=None/> }
}

/// The page as the editor loads it: title, Markdown, and the version it was read at.
#[derive(Clone)]
struct Draft {
    title: String,
    markdown: String,
    version: i64,
}

async fn load(id: &str) -> api::ApiResult<Draft> {
    let page: Value = api::get(&format!(
        "{}/pages/{id}?body-format=atlas_doc_format",
        api::WIKI2
    ))
    .await?;
    let adf = page["body"]["atlas_doc_format"]["value"]
        .as_str()
        .unwrap_or_default();
    let markdown = serde_json::from_str::<tessera_adf::Node>(adf)
        .map(|doc| tessera_adf::to_markdown(&doc))
        .unwrap_or_default();
    Ok(Draft {
        title: page["title"].as_str().unwrap_or_default().to_string(),
        markdown,
        version: page["version"]["number"].as_i64().unwrap_or(1),
    })
}

#[component]
fn Editor(id: Option<String>, parent: Option<String>) -> impl IntoView {
    let session = use_session();
    let space = use_space();
    let navigate = use_navigate();
    let title = RwSignal::new(String::new());
    let markdown = RwSignal::new(String::new());
    let version = RwSignal::new(0i64);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let editing = StoredValue::new(id.clone());

    // An edit starts from the saved page.
    if let Some(id) = id.clone() {
        spawn_local(async move {
            match load(&id).await {
                Ok(draft) => {
                    title.set(draft.title);
                    markdown.set(draft.markdown);
                    version.set(draft.version);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    }

    let cancel = {
        let navigate = navigate.clone();
        move |_| {
            let key = space.key.get_untracked();
            let target = match editing.get_value() {
                Some(id) => page_path(&key, &id),
                None => format!("/wiki/spaces/{key}"),
            };
            navigate(&target, Default::default());
        }
    };
    let publish = move |_| {
        let Some(found) = space.space.get_untracked().flatten() else {
            return;
        };
        saving.set(true);
        let navigate = navigate.clone();
        let parent = parent.clone();
        spawn_local(async move {
            let body = adf_body(&markdown.get_untracked());
            let result = match editing.get_value() {
                Some(id) => {
                    let update = json!({
                        "id": id,
                        "status": "current",
                        "title": title.get_untracked(),
                        "body": body,
                        "version": { "number": version.get_untracked() + 1 },
                    });
                    api::request(
                        gloo_net::http::Method::PUT,
                        &format!("{}/pages/{id}", api::WIKI2),
                        Some(&update),
                    )
                    .await
                }
                None => {
                    let mut create = json!({
                        "spaceId": found.id,
                        "status": "current",
                        "title": title.get_untracked(),
                        "body": body,
                    });
                    if let Some(parent) = parent {
                        create["parentId"] = json!(parent);
                    }
                    api::post::<Value>(&format!("{}/pages", api::WIKI2), &create).await
                }
            };
            saving.set(false);
            match result {
                Ok(page) => {
                    session.changed();
                    let id = page["id"].as_str().unwrap_or_default().to_string();
                    navigate(&page_path(&found.key, &id), Default::default());
                }
                Err(e) if e.status == 409 => error.set(Some(format!(
                    "Someone else saved this page while you were editing. Copy your text, reload, and try again. ({})",
                    e.message
                ))),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="page wiki-page editor-page">
            <input class="input title-input" placeholder="Give this page a title" node_ref=autofocus()
                prop:value=move || title.get()
                on:input:target=move |ev| title.set(ev.target().value())/>
            <textarea class="input editor wiki-editor" rows="24"
                placeholder="Write in Markdown: # headings, **bold**, *italic*, lists, `code`, ``` code blocks ```, and > quotes."
                prop:value=move || markdown.get()
                on:input:target=move |ev| markdown.set(ev.target().value())></textarea>
            <p class="muted small">"Panels, status lozenges, and other macros are kept as their text when you edit in Markdown."</p>
            <ErrorText error=error/>
            <div class="form-actions">
                <button class="button subtle" on:click=cancel>"Cancel"</button>
                <button class="button primary" disabled=move || saving.get() on:click=publish>
                    {move || if editing.get_value().is_some() { "Update" } else { "Publish" }}
                </button>
            </div>
        </div>
    }
}
