//! The Tessera web app. It runs in the browser as WebAssembly and talks to the server only
//! through the public REST API, like any other client.

mod api;
mod app;
mod components;
mod model;
mod pages;

fn main() {
    leptos::mount::mount_to_body(app::App);
}
