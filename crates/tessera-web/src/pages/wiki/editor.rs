//! The editor, for new pages, blog posts, and edits. Content is written in Markdown and saved
//! as ADF; the server keeps it in Confluence's storage format.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate, use_params_map, use_query_map};
use serde_json::{Value, json};

use super::{adf_body, collection, content_path, kind_of_path, use_space};
use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, MarkdownArea, autofocus};

/// `/wiki/spaces/:key/create?parent=ID`: a new page under `parent`, or under the home page.
#[component]
pub fn CreatePage() -> impl IntoView {
    let query = use_query_map();
    let parent = query.with_untracked(|q| q.get("parent"));
    view! { <Editor id=None parent=parent kind="page"/> }
}

/// `/wiki/spaces/:key/blog/create`.
#[component]
pub fn CreateBlogPost() -> impl IntoView {
    view! { <Editor id=None parent=None kind="blogpost"/> }
}

/// `/wiki/spaces/:key/pages/:id/edit` and `/wiki/spaces/:key/blog/:id/edit`.
#[component]
pub fn EditPage() -> impl IntoView {
    let params = use_params_map();
    let id = params.with_untracked(|p| p.get("id"));
    let kind = kind_of_path(&use_location().pathname.get_untracked());
    view! { <Editor id=id parent=None kind=kind/> }
}

/// The page as the editor loads it: title, Markdown, and the version it was read at.
#[derive(Clone)]
struct Draft {
    title: String,
    markdown: String,
    version: i64,
}

async fn load(id: &str, kind: &str) -> api::ApiResult<Draft> {
    let page: Value = api::get(&format!(
        "{}/{}/{id}?body-format=atlas_doc_format",
        api::WIKI2,
        collection(kind)
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
fn Editor(id: Option<String>, parent: Option<String>, kind: &'static str) -> impl IntoView {
    let session = use_session();
    let space = use_space();
    let navigate = use_navigate();
    let title = RwSignal::new(String::new());
    let markdown = RwSignal::new(String::new());
    let version = RwSignal::new(0i64);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let editing = StoredValue::new(id.clone());
    // Labels the chosen template gives the new page.
    let template_labels = RwSignal::new(Vec::<String>::new());

    // An edit starts from the saved page.
    if let Some(id) = id.clone() {
        spawn_local(async move {
            match load(&id, kind).await {
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
                Some(id) => content_path(&key, kind, &id),
                None if kind == "blogpost" => format!("/wiki/spaces/{key}/blog"),
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
                        &format!("{}/{}/{id}", api::WIKI2, collection(kind)),
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
                    api::post::<Value>(&format!("{}/{}", api::WIKI2, collection(kind)), &create)
                        .await
                }
            };
            saving.set(false);
            match result {
                Ok(page) => {
                    let id = page["id"].as_str().unwrap_or_default().to_string();
                    let labels = template_labels.get_untracked();
                    if editing.get_value().is_none() && !labels.is_empty() {
                        let names: Vec<Value> = labels
                            .iter()
                            .map(|name| json!({ "prefix": "global", "name": name }))
                            .collect();
                        let path = format!("{}/content/{id}/label", api::WIKI);
                        if let Err(e) = api::post::<Value>(&path, &json!(names)).await {
                            error.set(Some(e.message));
                        }
                    }
                    session.changed();
                    navigate(&content_path(&found.key, kind, &id), Default::default());
                }
                Err(e) if e.status == 409 => error.set(Some(format!(
                    "Someone else saved this page while you were editing. Copy your text, reload, and try again. ({})",
                    e.message
                ))),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let starting = editing.get_value().is_none() && kind == "page";
    let on_template = Callback::new(move |(text, labels): (String, Vec<String>)| {
        markdown.set(text);
        template_labels.set(labels);
    });
    view! {
        <div class="page wiki-page editor-page">
            {starting.then(|| view! { <TemplatePicker on_pick=on_template/> })}
            <input class="input title-input" node_ref=autofocus()
                placeholder=if kind == "blogpost" { "Give this blog post a title" } else { "Give this page a title" }
                prop:value=move || title.get()
                on:input:target=move |ev| title.set(ev.target().value())/>
            <MarkdownArea value=markdown rows=24 class="input editor wiki-editor"
                placeholder="Write in Markdown: # headings, **bold**, *italic*, lists, tables, - [ ] tasks, `code`, ``` code blocks ```, > quotes, ![alt](file.png) for an attached image, and @ to mention someone."/>
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

/// Loads a template's body as Markdown, with the labels it gives pages.
async fn load_template(id: &str) -> api::ApiResult<(String, Vec<String>)> {
    let template: Value = api::get(&format!("{}/template/{id}", api::WIKI)).await?;
    let storage = template["body"]["storage"]["value"]
        .as_str()
        .unwrap_or_default();
    let converted: Value = api::post(
        &format!("{}/contentbody/convert/atlas_doc_format", api::WIKI),
        &json!({ "value": storage, "representation": "storage" }),
    )
    .await?;
    let markdown = converted["value"]
        .as_str()
        .and_then(|adf| serde_json::from_str::<tessera_adf::Node>(adf).ok())
        .map(|doc| tessera_adf::to_markdown(&doc))
        .unwrap_or_default();
    let labels = template["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(str::to_string))
        .collect();
    Ok((markdown, labels))
}

/// Starts a new page from a template: the space's own, global ones, or Tessera's blueprints.
#[component]
fn TemplatePicker(on_pick: Callback<(String, Vec<String>)>) -> impl IntoView {
    let space = use_space();
    let error = RwSignal::new(None::<String>);
    let groups = LocalResource::new(move || {
        let key = space.key.get();
        async move {
            let list = |path: String| async move {
                api::get::<Value>(&path)
                    .await
                    .ok()
                    .and_then(|v| v["results"].as_array().cloned())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| {
                        (
                            t["templateId"].as_str().unwrap_or_default().to_string(),
                            t["name"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            vec![
                (
                    "This space",
                    list(format!("{}/template/page?spaceKey={key}", api::WIKI)).await,
                ),
                ("Global", list(format!("{}/template/page", api::WIKI)).await),
                (
                    "Blueprints",
                    list(format!("{}/template/blueprint", api::WIKI)).await,
                ),
            ]
        }
    });
    let pick = move |id: String| {
        if id.is_empty() {
            on_pick.run((String::new(), Vec::new()));
            return;
        }
        spawn_local(async move {
            match load_template(&id).await {
                Ok(picked) => {
                    error.set(None);
                    on_pick.run(picked);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="template-picker">
            <label class="small muted">"Start from"
                <select class="select" aria-label="Template" on:change:target=move |ev| pick(ev.target().value())>
                    <option value="">"Blank page"</option>
                    {move || groups.get().map(|groups| groups.into_iter().filter(|(_, items)| !items.is_empty()).map(|(label, items)| view! {
                        <optgroup label=label>
                            {items.into_iter().map(|(id, name)| view! { <option value=id>{name}</option> }).collect_view()}
                        </optgroup>
                    }).collect_view())}
                </select>
            </label>
            <ErrorText error=error/>
        </div>
    }
}
