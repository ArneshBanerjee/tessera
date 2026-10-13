//! Test helpers: an app on an in-memory database, an admin with an API token, and request helpers.
//! Each test binary uses a different subset of them.
#![allow(dead_code)]

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use serde_json::Value;
use tessera_server::store::users::{self, NewUser};
use tessera_server::{AppState, app, auth, db};
use tower::ServiceExt;

pub const BASE: &str = "http://tessera.test";

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    /// Basic auth header for the admin user.
    pub admin: String,
}

pub fn basic(email: &str, secret: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{secret}"));
    format!("Basic {encoded}")
}

impl TestApp {
    pub async fn new() -> Self {
        let pool = db::connect("sqlite::memory:").await.expect("database");
        let admin = users::create(
            &pool,
            NewUser {
                email: "admin@tessera.test",
                display_name: "Ada Admin",
                password_hash: Some(auth::hash_password("secret")),
                is_admin: true,
            },
        )
        .await
        .expect("admin");
        let token = auth::create_api_token(&pool, admin.id, "tests")
            .await
            .expect("token");
        let data_dir =
            std::env::temp_dir().join(format!("tessera-test-{}", uuid::Uuid::new_v4().simple()));
        let state = AppState {
            db: pool,
            base_url: BASE.into(),
            data_dir,
        };
        Self {
            router: app(state.clone()),
            state,
            admin: basic("admin@tessera.test", &token),
        }
    }

    /// Creates a regular user and returns its basic auth header and account id.
    pub async fn user(&self, email: &str, name: &str) -> (String, String) {
        let user = users::create(
            &self.state.db,
            NewUser {
                email,
                display_name: name,
                password_hash: Some(auth::hash_password("pw")),
                is_admin: false,
            },
        )
        .await
        .expect("user");
        (basic(email, "pw"), user.account_id)
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        auth: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(auth) = auth {
            request = request.header(header::AUTHORIZATION, auth);
        }
        let body = match body {
            Some(json) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        (status, json)
    }

    /// A request with any headers and body; returns status, headers, and raw bytes.
    pub async fn raw(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, self.admin.clone());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, bytes.to_vec())
    }

    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.request(Method::GET, path, Some(&self.admin.clone()), None)
            .await
    }

    pub async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::POST, path, Some(&self.admin.clone()), Some(body))
            .await
    }

    pub async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PUT, path, Some(&self.admin.clone()), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str) -> (StatusCode, Value) {
        self.request(Method::DELETE, path, Some(&self.admin.clone()), None)
            .await
    }

    /// Creates a project and returns its key.
    pub async fn project(&self, key: &str) -> String {
        let (status, body) = self
            .post(
                "/rest/api/3/project",
                serde_json::json!({ "key": key, "name": format!("Project {key}") }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        key.to_string()
    }

    /// Creates an issue with extra fields and returns its key.
    pub async fn issue(&self, project: &str, summary: &str, extra: Value) -> String {
        let mut fields = serde_json::json!({
            "project": { "key": project },
            "issuetype": { "name": "Task" },
            "summary": summary,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            fields[key] = value;
        }
        let (status, body) = self
            .post("/rest/api/3/issue", serde_json::json!({ "fields": fields }))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["key"].as_str().unwrap().to_string()
    }

    /// Runs JQL through `/search` and returns the matching issue keys in order.
    pub async fn jql(&self, jql: &str) -> Vec<String> {
        let (status, body) = self
            .post(
                "/rest/api/3/search",
                serde_json::json!({ "jql": jql, "fields": ["summary"] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{jql}: {body}");
        body["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_string())
            .collect()
    }
}
