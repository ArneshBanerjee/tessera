//! `/jira/dashboards`: the dashboards a user can see; `/jira/dashboards/:id`: one dashboard,
//! its gadgets in columns.

pub mod gadgets;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal};
use crate::model::{Page, User};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_favourite: bool,
    pub owner: Option<User>,
    pub is_writable: bool,
    pub system_dashboard: bool,
    pub layout: String,
    pub share_permissions: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Position {
    pub row: i64,
    pub column: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Gadget {
    pub id: i64,
    pub module_key: Option<String>,
    pub color: String,
    pub position: Position,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct GadgetList {
    gadgets: Vec<Gadget>,
}

pub fn dashboard_path(id: &str) -> String {
    format!("/jira/dashboards/{id}")
}

/// Who a dashboard is shared with, in words.
fn shared_with(dashboard: &Dashboard) -> String {
    if dashboard.system_dashboard {
        return "Everyone".into();
    }
    let names: Vec<String> = dashboard
        .share_permissions
        .iter()
        .map(|s| match s["type"].as_str() {
            Some("global") => "Everyone".to_string(),
            Some("loggedin") => "Signed-in users".to_string(),
            Some("project") => format!(
                "Project: {}",
                s["project"]["name"].as_str().unwrap_or_default()
            ),
            other => other.unwrap_or_default().to_string(),
        })
        .collect();
    if names.is_empty() {
        "Private".into()
    } else {
        names.join(", ")
    }
}

/// The share setting the dialog offers: private, signed-in users, or everyone.
fn share_choice(dashboard: Option<&Dashboard>) -> &'static str {
    let first = dashboard.and_then(|d| d.share_permissions.first());
    match first.and_then(|s| s["type"].as_str()) {
        Some("global") => "global",
        Some("loggedin") => "loggedin",
        _ => "private",
    }
}

/// Stars or unstars a dashboard.
async fn set_favourite(id: &str, favourite: bool) -> api::ApiResult<()> {
    let path = format!("{}/dashboard/{id}/favourite", api::ACCOUNT);
    let method = if favourite {
        gloo_net::http::Method::PUT
    } else {
        gloo_net::http::Method::DELETE
    };
    api::request(method, &path, None).await.map(|_| ())
}

#[component]
fn Star(id: String, favourite: bool) -> impl IntoView {
    let session = use_session();
    let id = StoredValue::new(id);
    let toggle = move |_| {
        spawn_local(async move {
            if set_favourite(&id.get_value(), !favourite).await.is_ok() {
                session.changed();
            }
        });
    };
    let label = if favourite { "Unstar" } else { "Star" };
    view! {
        <button class="star-button" class:starred=favourite aria-label=label title=label on:click=toggle>
            {if favourite { "★" } else { "☆" }}
        </button>
    }
}

#[component]
pub fn DashboardsPage() -> impl IntoView {
    let session = use_session();
    let creating = RwSignal::new(false);
    let list = LocalResource::new(move || {
        session.changes.track();
        async move {
            api::get::<Page<Dashboard>>(&format!("{}/dashboard/search?maxResults=100", api::CORE))
                .await
        }
    });
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Dashboards"</h1>
                <button class="button primary" on:click=move |_| creating.set(true)>"Create dashboard"</button>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || list.get().map(|result| match result {
                    Ok(page) => view! {
                        <table class="issue-table">
                            <thead><tr><th></th><th>"Name"</th><th>"Owner"</th><th>"Shared with"</th></tr></thead>
                            <tbody>
                                {page.values.into_iter().map(|d| {
                                    let shared = shared_with(&d);
                                    let owner = d.owner.as_ref().map_or_else(|| "System".to_string(), |o| o.display_name.clone());
                                    view! {
                                        <tr>
                                            <td class="star-cell"><Star id=d.id.clone() favourite=d.is_favourite/></td>
                                            <td><A href=dashboard_path(&d.id)>{d.name.clone()}</A></td>
                                            <td>{owner}</td>
                                            <td class="muted">{shared}</td>
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
        <Show when=move || creating.get()>
            <DashboardDialog mode=DialogMode::Create on_close=Callback::new(move |()| creating.set(false))/>
        </Show>
    }
}

#[derive(Clone, PartialEq)]
enum DialogMode {
    Create,
    Copy(Dashboard),
    Edit(Dashboard),
}

/// Creates, copies, or edits a dashboard's name, description, and sharing.
#[component]
fn DashboardDialog(mode: DialogMode, on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let existing = match &mode {
        DialogMode::Create => None,
        DialogMode::Copy(d) | DialogMode::Edit(d) => Some(d.clone()),
    };
    let name = RwSignal::new(match &mode {
        DialogMode::Create => String::new(),
        DialogMode::Copy(d) => format!("Copy of {}", d.name),
        DialogMode::Edit(d) => d.name.clone(),
    });
    let description = RwSignal::new(
        existing
            .as_ref()
            .map(|d| d.description.clone())
            .unwrap_or_default(),
    );
    let share = RwSignal::new(share_choice(existing.as_ref()).to_string());
    let error = RwSignal::new(None::<String>);
    let title = match &mode {
        DialogMode::Create => "Create dashboard",
        DialogMode::Copy(_) => "Copy dashboard",
        DialogMode::Edit(_) => "Edit dashboard",
    };
    let mode = StoredValue::new(mode);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let shares = match share.get().as_str() {
            "private" => json!([]),
            other => json!([{ "type": other }]),
        };
        let body = json!({ "name": name.get(), "description": description.get(), "sharePermissions": shares });
        let navigate = navigate.clone();
        spawn_local(async move {
            let result = match mode.get_value() {
                DialogMode::Create => {
                    api::post::<Dashboard>(&format!("{}/dashboard", api::CORE), &body).await
                }
                DialogMode::Copy(d) => {
                    api::post::<Dashboard>(&format!("{}/dashboard/{}/copy", api::CORE, d.id), &body)
                        .await
                }
                DialogMode::Edit(d) => {
                    api::put::<Dashboard>(&format!("{}/dashboard/{}", api::CORE, d.id), &body).await
                }
            };
            match result {
                Ok(saved) => {
                    session.changed();
                    on_close.run(());
                    navigate(&dashboard_path(&saved.id), Default::default());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let is_system = existing.as_ref().is_some_and(|d| d.system_dashboard)
        && matches!(mode.get_value(), DialogMode::Edit(_));
    view! {
        <Modal title=title.to_string() on_close=on_close>
            <form class="form" on:submit=save>
                <label>"Name"
                    <input class="input" required prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())/>
                </label>
                <label>"Description"
                    <textarea class="input" rows="2" prop:value=move || description.get()
                        on:input:target=move |ev| description.set(ev.target().value())></textarea>
                </label>
                {(!is_system).then(|| view! {
                    <label>"Shared with"
                        <select class="select" on:change:target=move |ev| share.set(ev.target().value())>
                            <option value="private" selected=move || share.get() == "private">"Private"</option>
                            <option value="loggedin" selected=move || share.get() == "loggedin">"Signed-in users"</option>
                            <option value="global" selected=move || share.get() == "global">"Everyone"</option>
                        </select>
                    </label>
                })}
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" type="submit">"Save"</button>
                    <button class="button subtle" type="button" on:click=move |_| on_close.run(())>"Cancel"</button>
                </div>
            </form>
        </Modal>
    }
}

/// CSS column widths for a layout: `A` is a normal column, `B` a wide one.
fn grid_columns(layout: &str) -> String {
    let widths: Vec<&str> = layout
        .chars()
        .map(|c| if c == 'B' { "2fr" } else { "1fr" })
        .collect();
    if widths.is_empty() {
        "1fr 1fr".into()
    } else {
        widths.join(" ")
    }
}

#[component]
pub fn DashboardPage() -> impl IntoView {
    let session = use_session();
    let params = use_params_map();
    let id = Signal::derive(move || params.with(|p| p.get("id")).unwrap_or_default());
    let loaded = LocalResource::new(move || {
        let id = id.get();
        session.changes.track();
        async move {
            let dashboard = api::get::<Dashboard>(&format!("{}/dashboard/{id}", api::CORE)).await?;
            let list =
                api::get::<GadgetList>(&format!("{}/dashboard/{id}/gadget", api::CORE)).await?;
            Ok::<_, api::ApiError>((dashboard, list.gadgets))
        }
    });
    view! {
        <div class="page dashboard-page">
            <Transition fallback=|| view! { <p class="muted">"Loading dashboard…"</p> }>
                {move || loaded.get().map(|result| match result {
                    Ok((dashboard, gadgets)) => view! { <DashboardView dashboard=dashboard gadgets=gadgets/> }.into_any(),
                    Err(e) => view! { <div class="empty"><h1>"Dashboard not found"</h1><p>{e.message}</p></div> }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    None,
    Edit,
    Copy,
    AddGadget,
}

#[component]
fn DashboardView(dashboard: Dashboard, gadgets: Vec<Gadget>) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let dialog = RwSignal::new(Dialog::None);
    let error = RwSignal::new(None::<String>);
    let confirming = RwSignal::new(false);
    let columns = dashboard.layout.len().max(1) as i64;
    let writable = dashboard.is_writable;
    let stored = StoredValue::new(dashboard.clone());
    let set_layout = move |layout: String| {
        let id = stored.get_value().id;
        spawn_local(async move {
            let path = format!("{}/dashboard/{id}/layout", api::ACCOUNT);
            match api::put::<Value>(&path, &json!({ "layout": layout })).await {
                Ok(_) => session.changed(),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let delete = move |_| {
        let id = stored.get_value().id;
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::delete(&format!("{}/dashboard/{id}", api::CORE)).await {
                Ok(()) => {
                    session.changed();
                    navigate("/jira/dashboards", Default::default());
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let layout = dashboard.layout.clone();
    view! {
        <header class="page-header dashboard-header">
            <div class="dashboard-title">
                <Star id=dashboard.id.clone() favourite=dashboard.is_favourite/>
                <h1>{dashboard.name.clone()}</h1>
            </div>
            <div class="issue-actions">
                {writable.then(|| view! {
                    <button class="button primary" on:click=move |_| dialog.set(Dialog::AddGadget)>"Add gadget"</button>
                    <select class="select layout-select" aria-label="Layout"
                        on:change:target=move |ev| set_layout(ev.target().value())>
                        {[("A", "One column"), ("AA", "Two columns"), ("AB", "Narrow and wide"), ("BA", "Wide and narrow"), ("AAA", "Three columns")]
                            .into_iter()
                            .map(|(value, label)| view! { <option value=value selected=layout == value>{label}</option> })
                            .collect_view()}
                    </select>
                    <button class="button" on:click=move |_| dialog.set(Dialog::Edit)>"Edit"</button>
                })}
                <button class="button" on:click=move |_| dialog.set(Dialog::Copy)>"Copy"</button>
                {(writable && !dashboard.system_dashboard).then(|| view! {
                    <Show when=move || confirming.get()
                        fallback=move || view! { <button class="button subtle" on:click=move |_| confirming.set(true)>"Delete"</button> }>
                        <span class="confirm">
                            "Delete this dashboard?"
                            <button class="button danger small" on:click=delete.clone()>"Delete"</button>
                            <button class="button subtle small" on:click=move |_| confirming.set(false)>"Cancel"</button>
                        </span>
                    </Show>
                })}
            </div>
        </header>
        {(!dashboard.description.is_empty()).then(|| view! { <p class="muted dashboard-description">{dashboard.description.clone()}</p> })}
        <ErrorText error=error/>
        <div class="dashboard-grid" style=format!("grid-template-columns: {}", grid_columns(&dashboard.layout))>
            {(0..columns).map(|col| {
                let mut in_column: Vec<Gadget> = gadgets.iter().filter(|g| g.position.column == col).cloned().collect();
                in_column.sort_by_key(|g| g.position.row);
                let size = in_column.len() as i64;
                view! {
                    <div class="dashboard-column">
                        {in_column.into_iter().map(|gadget| view! {
                            <gadgets::GadgetFrame dashboard=dashboard.id.clone() gadget=gadget
                                writable=writable columns=columns column_size=size/>
                        }).collect_view()}
                        {(size == 0 && writable).then(|| view! { <p class="drop-hint">"Add a gadget, then move it here."</p> })}
                    </div>
                }
            }).collect_view()}
        </div>
        {move || match dialog.get() {
            Dialog::None => None,
            Dialog::Edit => Some(view! { <DashboardDialog mode=DialogMode::Edit(stored.get_value()) on_close=Callback::new(move |()| dialog.set(Dialog::None))/> }.into_any()),
            Dialog::Copy => Some(view! { <DashboardDialog mode=DialogMode::Copy(stored.get_value()) on_close=Callback::new(move |()| dialog.set(Dialog::None))/> }.into_any()),
            Dialog::AddGadget => Some(view! { <gadgets::AddGadget dashboard=stored.get_value().id on_close=Callback::new(move |()| dialog.set(Dialog::None))/> }.into_any()),
        }}
    }
}
