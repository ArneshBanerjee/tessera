//! Search across Jira and Confluence.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

/// The titles in one section of a search.
fn titles(found: &Value, key: &str) -> Vec<String> {
    found["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap()["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn one_search_finds_issues_pages_and_more() {
    let app = TestApp::new().await;
    app.post(
        "/rest/api/3/project",
        json!({ "key": "ROCK", "name": "Rocket launch" }),
    )
    .await;
    let key = app.issue("ROCK", "Rocket fuel check", json!({})).await;
    app.issue("ROCK", "Unrelated chore", json!({})).await;
    app.post(
        "/wiki/rest/api/space",
        json!({ "key": "ROCKET", "name": "Rocket docs" }),
    )
    .await;
    let (_, page) = app
        .post(
            "/wiki/rest/api/content",
            json!({ "type": "page", "title": "Rocket checklist", "space": { "key": "ROCKET" },
                    "body": { "storage": { "value": "<p>Steps</p>", "representation": "storage" } } }),
        )
        .await;
    app.post(
        "/rest/api/3/filter",
        json!({ "name": "Rocket bugs", "jql": "project = ROCK" }),
    )
    .await;

    let (status, found) = app.get("/rest/tessera/1.0/search?query=rocket").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(
        titles(&found, "issues"),
        [format!("{key} Rocket fuel check")]
    );
    // The space's home page matches too.
    assert!(titles(&found, "pages").contains(&"Rocket checklist".to_string()));
    assert_eq!(titles(&found, "spaces"), ["Rocket docs"]);
    assert_eq!(titles(&found, "projects"), ["Rocket launch"]);
    assert_eq!(titles(&found, "filters"), ["Rocket bugs"]);

    // An issue key finds that issue first.
    let (_, found) = app
        .get(&format!("/rest/tessera/1.0/search?query={key}"))
        .await;
    assert_eq!(
        titles(&found, "issues")[0],
        format!("{key} Rocket fuel check")
    );

    // Restricted pages stay hidden from people outside the restriction.
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let (_, me) = app.get("/rest/api/3/myself").await;
    let id = page["id"].as_str().unwrap();
    app.put(
        &format!("/wiki/rest/api/content/{id}/restriction"),
        json!([{ "operation": "read", "restrictions": { "user": [{ "accountId": me["accountId"] }] } }]),
    )
    .await;
    let (_, found) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/search?query=rocket",
            Some(&dev),
            None,
        )
        .await;
    assert!(!titles(&found, "pages").contains(&"Rocket checklist".to_string()));
    assert_eq!(titles(&found, "issues").len(), 1);

    // Quotes and odd text do not break the search.
    let (status, _) = app
        .get("/rest/tessera/1.0/search?query=%22rock%5C%22et")
        .await;
    assert_eq!(status, StatusCode::OK);
}
