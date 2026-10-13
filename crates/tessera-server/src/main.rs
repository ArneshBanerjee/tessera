use std::net::SocketAddr;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct Config {
    /// The public URL clients use to reach this server.
    base_url: String,
}

/// The `ServerInformation` object from the public Jira Cloud REST API v3 spec.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerInfo {
    base_url: String,
    display_url: String,
    /// Tessera implements the Jira Cloud REST API, so it reports the Cloud deployment type and
    /// version line. Clients use these to pick API behavior (for example, account IDs over usernames).
    deployment_type: &'static str,
    version: &'static str,
    version_numbers: [u32; 3],
    build_number: u32,
    /// Tessera's own version.
    scm_info: String,
    server_time: String,
    server_time_zone: &'static str,
    server_title: &'static str,
}

async fn server_info(State(config): State<Config>) -> Json<ServerInfo> {
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    Json(ServerInfo {
        base_url: config.base_url.clone(),
        display_url: config.base_url,
        deployment_type: "Cloud",
        version: "1001.0.0",
        version_numbers: [1001, 0, 0],
        build_number: 1,
        scm_info: format!("tessera-{}", env!("CARGO_PKG_VERSION")),
        server_time: now.strftime("%Y-%m-%dT%H:%M:%S%.3f%z").to_string(),
        server_time_zone: "UTC",
        server_title: "Tessera",
    })
}

fn app(config: Config) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/rest/api/2/serverInfo", get(server_info))
        .route("/rest/api/3/serverInfo", get(server_info))
        .with_state(config)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = std::env::var("TESSERA_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("TESSERA_ADDR must be a socket address, for example 127.0.0.1:8080");
    let base_url = std::env::var("TESSERA_BASE_URL").unwrap_or_else(|_| format!("http://{addr}"));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Tessera listening on {base_url}");
    axum::serve(listener, app(Config { base_url }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn server_info_matches_the_jira_schema() {
        for path in ["/rest/api/2/serverInfo", "/rest/api/3/serverInfo"] {
            let app = app(Config {
                base_url: "http://tessera.test".into(),
            });
            let response = app
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");

            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["baseUrl"], "http://tessera.test");
            assert_eq!(json["deploymentType"], "Cloud");
            assert_eq!(json["versionNumbers"], serde_json::json!([1001, 0, 0]));
            // Jira timestamps look like 2026-10-13T09:30:00.000+0000.
            let time = json["serverTime"].as_str().unwrap();
            assert!(time.len() == 28 && time.ends_with("+0000"), "{time}");
        }
    }
}
