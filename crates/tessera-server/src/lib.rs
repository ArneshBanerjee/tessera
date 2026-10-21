//! The Tessera server.
//!
//! `main.rs` is a thin command-line wrapper. Integration tests build the app from this library.

pub mod api;
pub mod auth;
pub mod confluence;
pub mod db;
pub mod error;
pub mod import;
pub mod jql;
pub mod mail;
pub mod notify;
pub mod reports;
pub mod store;
pub mod time;
mod web;

use std::path::PathBuf;

use axum::Router;
use sqlx::SqlitePool;

/// Shared state for every request.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    /// The public URL clients use to reach this server, without a trailing slash.
    pub base_url: String,
    /// Where files such as attachments are stored.
    pub data_dir: PathBuf,
    /// Whether outgoing email is set up. Without it, no notifications are queued.
    pub mail_enabled: bool,
}

/// Builds the whole HTTP application.
pub fn app(state: AppState) -> Router {
    api::router()
        .merge(confluence::router())
        .fallback(web::fallback)
        .with_state(state)
}
