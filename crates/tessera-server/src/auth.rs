//! Authentication.
//!
//! - REST clients use HTTP Basic auth with an email and an API token, as with Jira Cloud.
//!   A password also works, which is convenient for a self-hosted server.
//! - The web app uses a session cookie, created by `POST /rest/auth/1/session`.
//!
//! Tokens and session ids are random; the database stores only their SHA-256 hashes.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use base64::Engine;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::AppState;
use crate::error::ApiError;
use crate::store::users::{self, User};
use crate::time::now_ms;

/// The name of the web app's session cookie.
pub const SESSION_COOKIE: &str = "tessera.session";
/// Sessions last 30 days.
const SESSION_TTL_MS: i64 = 30 * 24 * 3_600_000;

pub fn hash_password(password: &str) -> String {
    password_auth::generate_hash(password)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    password_auth::verify_password(password, hash).is_ok()
}

/// A random 256-bit secret, hex encoded.
fn random_secret() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    format!("{}{}", a.simple(), b.simple())
}

fn sha256_hex(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Creates an API token for a user and returns the secret. It is shown once and never stored.
pub async fn create_api_token(db: &SqlitePool, user_id: i64, label: &str) -> sqlx::Result<String> {
    let secret = random_secret();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, label, token_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(label)
    .bind(sha256_hex(&secret))
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(secret)
}

/// Creates a browser session and returns the cookie value.
pub async fn create_session(db: &SqlitePool, user_id: i64) -> sqlx::Result<String> {
    let secret = random_secret();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(sha256_hex(&secret))
        .bind(user_id)
        .bind(now_ms() + SESSION_TTL_MS)
        .execute(db)
        .await?;
    Ok(secret)
}

pub async fn delete_session(db: &SqlitePool, secret: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(sha256_hex(secret))
        .execute(db)
        .await?;
    Ok(())
}

/// Checks an email and a secret, which can be an API token or the password.
pub async fn check_credentials(
    db: &SqlitePool,
    email: &str,
    secret: &str,
) -> sqlx::Result<Option<User>> {
    let Some(user) = users::by_email(db, email).await? else {
        return Ok(None);
    };
    if !user.active {
        return Ok(None);
    }
    let token_matches =
        sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE user_id = ? AND token_hash = ?")
            .bind(now_ms())
            .bind(user.id)
            .bind(sha256_hex(secret))
            .execute(db)
            .await?
            .rows_affected()
            > 0;
    let password_matches = || {
        user.password_hash
            .as_deref()
            .is_some_and(|hash| verify_password(secret, hash))
    };
    Ok((token_matches || password_matches()).then_some(user))
}

async fn session_user(db: &SqlitePool, secret: &str) -> sqlx::Result<Option<User>> {
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash = ? AND expires_at > ?")
            .bind(sha256_hex(secret))
            .bind(now_ms())
            .fetch_optional(db)
            .await?;
    match user_id {
        Some(id) => Ok(users::by_id(db, id).await?.filter(|u| u.active)),
        None => Ok(None),
    }
}

/// Reads one cookie from the request headers.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}

/// The authenticated user, or `None` for an anonymous request.
pub async fn authenticate(parts: &Parts, db: &SqlitePool) -> Result<Option<User>, ApiError> {
    if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
        let encoded = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Basic "))
            .ok_or_else(ApiError::unauthorized)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(ApiError::unauthorized)?;
        let (email, secret) = decoded.split_once(':').ok_or_else(ApiError::unauthorized)?;
        // Wrong credentials are an error even where anonymous access is allowed, as in Jira.
        return check_credentials(db, email, secret)
            .await?
            .map(Some)
            .ok_or_else(ApiError::unauthorized);
    }
    if let Some(secret) = cookie(&parts.headers, SESSION_COOKIE) {
        return Ok(session_user(db, secret).await?);
    }
    Ok(None)
}

/// Extractor for handlers that need a signed-in user. Rejects anonymous requests with 401.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        authenticate(parts, &state.db)
            .await?
            .map(CurrentUser)
            .ok_or_else(ApiError::unauthorized)
    }
}

/// Extractor for handlers that require a site administrator.
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(ApiError::forbidden(
                "You must have global administrator rights in order to modify this.",
            ));
        }
        Ok(AdminUser(user))
    }
}
