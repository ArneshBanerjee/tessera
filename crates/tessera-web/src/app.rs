//! The application shell: session, login, top navigation, and routes.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, ParentRoute, Route, Router, Routes};
use leptos_router::hooks::use_navigate;
use leptos_router::path;
use serde_json::json;

use crate::api;
use crate::components::{Avatar, ErrorText};
use crate::model::User;
use crate::pages::{
    admin, admin_workflows, board, create, filters, issue, project_settings, projects, reports,
    search, settings, wiki, your_work,
};

/// Shared state for the signed-in user.
#[derive(Clone, Copy)]
pub struct Session {
    pub me: StoredValue<User>,
    /// Whether the create issue dialog is open.
    pub creating: RwSignal<bool>,
    /// Bumped after changes, so open lists reload.
    pub changes: RwSignal<u32>,
    /// Whether the signed-in user administers the site.
    pub admin: RwSignal<bool>,
}

impl Session {
    /// Tells every list on screen that data changed.
    pub fn changed(&self) {
        self.changes.update(|n| *n += 1);
    }
}

pub fn use_session() -> Session {
    expect_context::<Session>()
}

#[component]
pub fn App() -> impl IntoView {
    let me =
        LocalResource::new(|| async { api::get::<User>(&format!("{}/myself", api::CORE)).await });
    view! {
        <Router>
            <Transition fallback=|| view! { <div class="page-loading">"Loading Tessera…"</div> }>
                {move || {
                    me.get().map(|result| match result {
                        Ok(user) => view! { <Shell me=user/> }.into_any(),
                        Err(_) => view! { <Login on_login=Callback::new(move |()| me.refetch())/> }.into_any(),
                    })
                }}
            </Transition>
        </Router>
    }
}

#[component]
fn Login(on_login: Callback<()>) -> impl IntoView {
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "username": email.get(), "password": password.get() });
        spawn_local(async move {
            match api::post::<serde_json::Value>("/rest/auth/1/session", &body).await {
                Ok(_) => on_login.run(()),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <main class="login">
            <form class="login-card" on:submit=submit>
                <div class="brand brand-large"><span class="brand-mark"></span>"Tessera"</div>
                <h1>"Log in to continue"</h1>
                <label>"Email"
                    <input class="input" type="email" required autocomplete="username"
                        prop:value=move || email.get()
                        on:input:target=move |ev| email.set(ev.target().value())/>
                </label>
                <label>"Password"
                    <input class="input" type="password" required autocomplete="current-password"
                        prop:value=move || password.get()
                        on:input:target=move |ev| password.set(ev.target().value())/>
                </label>
                <ErrorText error=error/>
                <button class="button primary wide" type="submit">"Log in"</button>
            </form>
        </main>
    }
}

#[component]
fn Shell(me: User) -> impl IntoView {
    let session = Session {
        me: StoredValue::new(me),
        creating: RwSignal::new(false),
        changes: RwSignal::new(0),
        admin: RwSignal::new(false),
    };
    provide_context(session);
    spawn_local(async move {
        if let Ok(me) = api::get::<serde_json::Value>(&format!("{}/me", api::ACCOUNT)).await {
            session.admin.set(me["admin"].as_bool().unwrap_or(false));
        }
    });
    view! {
        <TopNav/>
        <div class="app-body">
            <Routes fallback=|| view! { <div class="page"><h1>"Page not found"</h1></div> }>
                <Route path=path!("/") view=your_work::YourWork/>
                <Route path=path!("/projects") view=projects::Projects/>
                <ParentRoute path=path!("/projects/:key") view=projects::ProjectLayout>
                    <Route path=path!("") view=board::ProjectHome/>
                    <Route path=path!("board") view=board::BoardPage/>
                    <Route path=path!("backlog") view=board::BacklogPage/>
                    <Route path=path!("issues") view=search::ProjectIssues/>
                    <Route path=path!("settings") view=project_settings::ProjectSettings/>
                    <Route path=path!("reports") view=reports::Reports/>
                </ParentRoute>
                <Route path=path!("/browse/:key") view=issue::IssuePage/>
                <Route path=path!("/issues") view=search::SearchPage/>
                <Route path=path!("/filters") view=filters::FiltersPage/>
                <Route path=path!("/settings") view=settings::Settings/>
                <Route path=path!("/admin/users") view=admin::Users/>
                <Route path=path!("/admin/workflows") view=admin_workflows::Workflows/>
                <Route path=path!("/wiki") view=wiki::Spaces/>
                <ParentRoute path=path!("/wiki/spaces/:key") view=wiki::SpaceLayout>
                    <Route path=path!("") view=wiki::SpaceHome/>
                    <Route path=path!("create") view=wiki::editor::CreatePage/>
                    <Route path=path!("settings") view=wiki::space_settings::SpaceSettings/>
                    <Route path=path!("pages/:id") view=wiki::page::PageView/>
                    <Route path=path!("pages/:id/edit") view=wiki::editor::EditPage/>
                    <Route path=path!("pages/:id/history") view=wiki::page::PageHistory/>
                </ParentRoute>
            </Routes>
        </div>
        <Show when=move || session.creating.get()>
            <create::CreateIssue/>
        </Show>
    }
}

#[component]
fn TopNav() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let query = RwSignal::new(String::new());
    // Quick search: an issue key opens the issue; anything else searches text.
    let search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = query.get().trim().to_string();
        if text.is_empty() {
            return;
        }
        let is_key = text.rsplit_once('-').is_some_and(|(project, number)| {
            !project.is_empty() && number.chars().all(|c| c.is_ascii_digit()) && !number.is_empty()
        });
        if is_key {
            navigate(
                &format!("/browse/{}", text.to_uppercase()),
                Default::default(),
            );
        } else {
            let jql = format!(
                "text ~ \"{}\" ORDER BY updated DESC",
                text.replace('"', "\\\"")
            );
            navigate(
                &format!("/issues?jql={}", api::encode(&jql)),
                Default::default(),
            );
        }
        query.set(String::new());
    };
    let logout = move |_| {
        spawn_local(async move {
            let _ = api::delete("/rest/auth/1/session").await;
            let _ = web_sys::window().map(|w| w.location().reload());
        });
    };
    view! {
        <nav class="topnav">
            <A href="/" attr:class="brand"><span class="brand-mark"></span>"Tessera"</A>
            <A href="/" attr:class="nav-link">"Your work"</A>
            <A href="/projects" attr:class="nav-link">"Projects"</A>
            <A href="/filters" attr:class="nav-link">"Filters"</A>
            <A href="/wiki" attr:class="nav-link">"Spaces"</A>
            <button class="button primary" on:click=move |_| session.creating.set(true)>"Create"</button>
            <form class="nav-search" on:submit=search>
                <input class="input" type="search" placeholder="Search or go to PROJ-1"
                    prop:value=move || query.get()
                    on:input:target=move |ev| query.set(ev.target().value())/>
            </form>
            <div class="nav-user">
                <Show when=move || session.admin.get()>
                    <A href="/admin/users" attr:class="nav-link">"Admin"</A>
                </Show>
                <A href="/settings" attr:class="nav-avatar" attr:title="Your profile and API tokens">
                    <Avatar user=Some(session.me.get_value()) size=28/>
                </A>
                <button class="button subtle" on:click=logout>"Log out"</button>
            </div>
        </nav>
    }
}
