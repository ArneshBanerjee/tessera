//! `GET /serverInfo`. Anyone may call it, as in Jira.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::AppState;
use crate::time::{format, now_ms};

pub async fn get(State(state): State<AppState>) -> Json<Value> {
    // Tessera implements the Jira Cloud REST API, so it reports the Cloud deployment type and
    // version line. Clients use these to pick API behavior, such as account ids over usernames.
    Json(json!({
        "baseUrl": state.base_url,
        "displayUrl": state.base_url,
        "deploymentType": "Cloud",
        "version": "1001.0.0",
        "versionNumbers": [1001, 0, 0],
        "buildNumber": 1,
        "scmInfo": format!("tessera-{}", env!("CARGO_PKG_VERSION")),
        "serverTime": format(now_ms()),
        "serverTimeZone": "UTC",
        "serverTitle": "Tessera",
    }))
}
