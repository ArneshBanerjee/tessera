//! `/admin/import`: restore Jira and Confluence backups.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use super::admin_workflows::AdminTabs;
use crate::api;
use crate::app::use_session;
use crate::components::ErrorText;

#[component]
pub fn Import() -> impl IntoView {
    view! {
        <div class="page settings-page">
            <header class="page-header"><h1>"Administration"</h1></header>
            <AdminTabs current="Import"/>
            <p class="muted">"Bring in a backup from Jira or Confluence: the zip from the backup manager, or its entities.xml. Keys, dates, authors, history, and attachments are kept. People get accounts without passwords; they set one by resetting it, or you set it for them. For very large backups, use the command line: "<code>"tessera import-jira backup.zip"</code>"."</p>
            <Restore product="jira" title="Jira backup"/>
            <Restore product="confluence" title="Confluence backup"/>
        </div>
    }
}

#[component]
fn Restore(product: &'static str, title: &'static str) -> impl IntoView {
    let session = use_session();
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let result = RwSignal::new(None::<Value>);
    let upload = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        let Some(file) = input.files().and_then(|f| f.get(0)) else {
            return;
        };
        let Ok(form) = web_sys::FormData::new() else {
            return;
        };
        let _ = form.append_with_blob_and_filename("file", &file, &file.name());
        busy.set(true);
        error.set(None);
        result.set(None);
        spawn_local(async move {
            let path = format!("{}/admin/import/{product}", api::ACCOUNT);
            let sent = match gloo_net::http::Request::post(&path)
                .header("X-Atlassian-Token", "no-check")
                .body(form)
            {
                Ok(request) => request.send().await,
                Err(e) => Err(e),
            };
            busy.set(false);
            match sent {
                Ok(response) => {
                    let ok = response.ok();
                    let body: Value = response.json().await.unwrap_or_default();
                    if ok {
                        result.set(Some(body));
                        session.changed();
                    } else {
                        let message = body["errorMessages"][0]
                            .as_str()
                            .unwrap_or("The import failed.")
                            .to_string();
                        error.set(Some(message));
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    view! {
        <section class="settings-card">
            <h2>{title}</h2>
            <label class="button">
                {move || if busy.get() { "Importing…" } else { "Choose a backup file" }}
                <input type="file" accept=".zip,.xml" class="visually-hidden" disabled=move || busy.get() on:change=upload/>
            </label>
            <ErrorText error=error/>
            {move || result.get().map(|body| view! {
                <div class="import-result" role="status">
                    <strong>"Imported"</strong>
                    <ul>
                        {body["counts"].as_array().cloned().unwrap_or_default().into_iter().map(|c| view! {
                            <li>{format!("{}: {}", c["what"].as_str().unwrap_or_default(), c["count"])}</li>
                        }).collect_view()}
                    </ul>
                    {body["notes"].as_array().cloned().unwrap_or_default().into_iter().map(|n| view! {
                        <p class="muted small">{n.as_str().unwrap_or_default().to_string()}</p>
                    }).collect_view()}
                </div>
            })}
        </section>
    }
}
