//! `/admin/security`: issue security schemes, their levels, and who belongs to each level.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::admin_workflows::AdminTabs;
use crate::api;
use crate::app::{Session, use_session};
use crate::components::{ErrorText, UserSelect, users_resource};
use crate::model::User;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Level {
    id: String,
    name: String,
    description: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Scheme {
    id: i64,
    name: String,
    description: String,
    default_security_level_id: Option<i64>,
    levels: Vec<Level>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Holder {
    #[serde(rename = "type")]
    kind: String,
    parameter: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Member {
    id: String,
    issue_security_level_id: String,
    holder: Holder,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Paged<T> {
    values: Vec<T>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Role {
    id: i64,
    name: String,
}

/// Member types an administrator can add: (type, label, what the parameter is).
const MEMBER_TYPES: &[(&str, &str, &str)] = &[
    ("reporter", "Reporter", ""),
    ("assignee", "Current assignee", ""),
    ("lead", "Project lead", ""),
    ("applicationRole", "Any signed-in user", ""),
    ("group", "Group", "group"),
    ("user", "Single user", "user"),
    ("projectrole", "Project role", "role"),
];

/// A member in words, such as "Group: staff".
fn describe(holder: &Holder, users: &[User], roles: &[Role]) -> String {
    let parameter = holder.parameter.clone().unwrap_or_default();
    match holder.kind.as_str() {
        "reporter" => "Reporter".into(),
        "assignee" => "Current assignee".into(),
        "projectLead" => "Project lead".into(),
        "applicationRole" => "Any signed-in user".into(),
        "anyone" => "Anyone".into(),
        "group" => format!("Group: {parameter}"),
        "user" => format!(
            "User: {}",
            users
                .iter()
                .find(|u| u.account_id == parameter)
                .map_or(parameter.clone(), |u| u.display_name.clone())
        ),
        "projectRole" => format!(
            "Project role: {}",
            roles
                .iter()
                .find(|r| r.id.to_string() == parameter)
                .map_or(parameter.clone(), |r| r.name.clone())
        ),
        other => other.to_string(),
    }
}

/// Sends a change and refreshes the page, or shows the error.
fn send(
    session: Session,
    method: gloo_net::http::Method,
    path: String,
    body: Option<Value>,
    error: RwSignal<Option<String>>,
) {
    spawn_local(async move {
        match api::request(method, &path, body.as_ref()).await {
            Ok(_) => {
                error.set(None);
                session.changed();
            }
            Err(e) => error.set(Some(e.message)),
        }
    });
}

#[component]
pub fn IssueSecurity() -> impl IntoView {
    let session = use_session();
    let error = RwSignal::new(None::<String>);
    let users = users_resource();
    let roles = LocalResource::new(|| async {
        api::get::<Vec<Role>>(&format!("{}/role", api::CORE))
            .await
            .unwrap_or_default()
    });
    let loaded = LocalResource::new(move || {
        session.changes.track();
        async move {
            let list: Value = api::get(&format!("{}/issuesecurityschemes", api::CORE)).await?;
            let mut schemes = Vec::new();
            for item in list["issueSecuritySchemes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                let id = item["id"].as_i64().unwrap_or_default();
                schemes.push(
                    api::get::<Scheme>(&format!("{}/issuesecurityschemes/{id}", api::CORE)).await?,
                );
            }
            let members: Paged<Member> = api::get(&format!(
                "{}/issuesecurityschemes/level/member?maxResults=100",
                api::CORE
            ))
            .await?;
            Ok::<_, api::ApiError>((schemes, members.values))
        }
    });
    let name = RwSignal::new(String::new());
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "name": name.get() });
        name.set(String::new());
        send(
            session,
            gloo_net::http::Method::POST,
            format!("{}/issuesecurityschemes", api::CORE),
            Some(body),
            error,
        );
    };
    view! {
        <div class="page settings-page wide-settings">
            <header class="page-header"><h1>"Administration"</h1></header>
            <AdminTabs current="Issue security"/>
            <p class="muted">"An issue at a security level is seen only by the level's members. Give a project a scheme in its settings."</p>
            <ErrorText error=error/>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || loaded.get().map(|result| match result {
                    Ok((schemes, members)) => schemes.into_iter().map(|scheme| {
                        let members = members.clone();
                        view! { <SchemeCard scheme=scheme members=members users=users roles=roles error=error/> }
                    }).collect_view().into_any(),
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
            <section class="settings-card">
                <h2>"New scheme"</h2>
                <form class="inline-form" on:submit=create>
                    <input class="input" required placeholder="Scheme name" prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())/>
                    <button class="button primary" type="submit">"Create"</button>
                </form>
            </section>
        </div>
    }
}

#[component]
fn SchemeCard(
    scheme: Scheme,
    members: Vec<Member>,
    users: LocalResource<Vec<User>>,
    roles: LocalResource<Vec<Role>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = use_session();
    let id = scheme.id;
    let level_name = RwSignal::new(String::new());
    let add_level = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "levels": [{ "name": level_name.get() }] });
        level_name.set(String::new());
        send(
            session,
            gloo_net::http::Method::PUT,
            format!("{}/issuesecurityschemes/{id}/level", api::CORE),
            Some(body),
            error,
        );
    };
    let delete_scheme = move |_| {
        send(
            session,
            gloo_net::http::Method::DELETE,
            format!("{}/issuesecurityschemes/{id}", api::CORE),
            None,
            error,
        );
    };
    let default_level = scheme.default_security_level_id;
    view! {
        <section class="settings-card">
            <div class="card-heading">
                <h2>{scheme.name.clone()}</h2>
                <button class="link-button small" on:click=delete_scheme>"Delete scheme"</button>
            </div>
            {(!scheme.description.is_empty()).then(|| view! { <p class="muted small">{scheme.description.clone()}</p> })}
            {if scheme.levels.is_empty() {
                view! { <p class="muted small">"No levels yet: projects with this scheme show every issue to everyone who can browse them."</p> }.into_any()
            } else {
                view! {
                    <table class="issue-table">
                        <thead><tr><th>"Level"</th><th>"Members"</th><th></th></tr></thead>
                        <tbody>
                            {scheme.levels.clone().into_iter().map(|level| {
                                let in_level: Vec<Member> = members.iter().filter(|m| m.issue_security_level_id == level.id).cloned().collect();
                                let is_default = default_level.is_some_and(|d| d.to_string() == level.id);
                                view! { <LevelRow scheme=id level=level members=in_level is_default=is_default users=users roles=roles error=error/> }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_any()
            }}
            <form class="inline-form" on:submit=add_level>
                <input class="input" required placeholder="New level name" prop:value=move || level_name.get()
                    on:input:target=move |ev| level_name.set(ev.target().value())/>
                <button class="button" type="submit">"Add level"</button>
            </form>
        </section>
    }
}

#[component]
fn LevelRow(
    scheme: i64,
    level: Level,
    members: Vec<Member>,
    is_default: bool,
    users: LocalResource<Vec<User>>,
    roles: LocalResource<Vec<Role>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = use_session();
    let level_id = level.id.clone();
    let base = StoredValue::new(format!(
        "{}/issuesecurityschemes/{scheme}/level/{level_id}",
        api::CORE
    ));
    let make_default = {
        let level_id = level_id.clone();
        move |_| {
            let body = json!({ "defaultValues": [{ "issueSecuritySchemeId": scheme.to_string(), "defaultLevelId": level_id }] });
            send(
                session,
                gloo_net::http::Method::PUT,
                format!("{}/issuesecurityschemes/level/default", api::CORE),
                Some(body),
                error,
            );
        }
    };
    let delete_level = move |_| {
        send(
            session,
            gloo_net::http::Method::DELETE,
            base.get_value(),
            None,
            error,
        );
    };
    let kind = RwSignal::new("reporter".to_string());
    let parameter = RwSignal::new(String::new());
    let add_member = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let mut member = json!({ "type": kind.get() });
        if !parameter.get().is_empty() {
            member["parameter"] = json!(parameter.get());
        }
        send(
            session,
            gloo_net::http::Method::PUT,
            format!("{}/member", base.get_value()),
            Some(json!({ "members": [member] })),
            error,
        );
    };
    let needs = move || {
        MEMBER_TYPES
            .iter()
            .find(|(k, _, _)| *k == kind.get())
            .map_or("", |(_, _, needs)| *needs)
    };
    view! {
        <tr>
            <td>
                <strong>{level.name.clone()}</strong>
                {is_default.then(|| view! { <span class="count default-badge">"Default"</span> })}
                {(!level.description.is_empty()).then(|| view! { <div class="muted small">{level.description.clone()}</div> })}
            </td>
            <td>
                <ul class="member-list">
                    {move || {
                        let users = users.get().unwrap_or_default();
                        let roles = roles.get().unwrap_or_default();
                        members.clone().into_iter().map(|m| {
                            let path = format!("{}/member/{}", base.get_value(), m.id);
                            view! {
                                <li>
                                    {describe(&m.holder, &users, &roles)}
                                    <button class="link-button small" on:click=move |_| send(session, gloo_net::http::Method::DELETE, path.clone(), None, error)>"Remove"</button>
                                </li>
                            }
                        }).collect_view()
                    }}
                </ul>
                <form class="inline-form member-form" on:submit=add_member>
                    <select class="select" aria-label="Member type"
                        on:change:target=move |ev| { kind.set(ev.target().value()); parameter.set(String::new()); }>
                        {MEMBER_TYPES.iter().map(|(value, label, _)| view! { <option value=*value>{*label}</option> }).collect_view()}
                    </select>
                    {move || match needs() {
                        "group" => Some(view! {
                            <input class="input" required placeholder="Group name" prop:value=move || parameter.get()
                                on:input:target=move |ev| parameter.set(ev.target().value())/>
                        }.into_any()),
                        "user" => Some(view! {
                            <UserSelect users=users selected=None on_change=Callback::new(move |id: Option<String>| parameter.set(id.unwrap_or_default()))/>
                        }.into_any()),
                        "role" => Some(view! {
                            <select class="select" aria-label="Project role" on:change:target=move |ev| parameter.set(ev.target().value())>
                                <option value="">"Pick a role"</option>
                                {move || roles.get().unwrap_or_default().into_iter().map(|r| view! { <option value=r.id.to_string()>{r.name}</option> }).collect_view()}
                            </select>
                        }.into_any()),
                        _ => None,
                    }}
                    <button class="button small" type="submit">"Add"</button>
                </form>
            </td>
            <td class="row-actions">
                {(!is_default).then(|| view! { <button class="link-button small" on:click=make_default.clone()>"Make default"</button> })}
                <button class="link-button small" on:click=delete_level>"Delete"</button>
            </td>
        </tr>
    }
}
