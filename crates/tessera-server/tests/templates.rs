//! Confluence page templates: blueprints, global templates, and space templates.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::json;

const API: &str = "/wiki/rest/api";

#[tokio::test]
async fn templates_by_space_and_blueprints() {
    let app = TestApp::new().await;
    app.post(
        &format!("{API}/space"),
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;

    // Blueprints come with Tessera and cannot be changed.
    let (_, blueprints) = app.get(&format!("{API}/template/blueprint")).await;
    let names: Vec<&str> = blueprints["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Meeting notes"), "{blueprints}");
    let meeting = blueprints["results"][0]["templateId"].as_str().unwrap();
    let (status, _) = app.delete(&format!("{API}/template/{meeting}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A space template, made by the space's administrator.
    let (status, created) = app
        .post(
            &format!("{API}/template"),
            json!({
                "name": "Runbook",
                "templateType": "page",
                "description": "For on-call",
                "space": { "key": "DOCS" },
                "labels": [{ "prefix": "global", "name": "runbook" }],
                "body": { "storage": { "value": "<h2>Symptoms</h2><p>What you see</p>", "representation": "storage" } },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["templateId"].as_str().unwrap().to_string();
    assert_eq!(created["space"]["key"], "DOCS");
    assert_eq!(created["labels"][0]["name"], "runbook");

    // Space members see it; the global list does not include it.
    let (_, in_space) = app
        .request(
            Method::GET,
            &format!("{API}/template/page?spaceKey=DOCS&expand=body"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(in_space["size"], 1);
    assert_eq!(
        in_space["results"][0]["body"]["storage"]["value"],
        "<h2>Symptoms</h2><p>What you see</p>"
    );
    let (_, global) = app.get(&format!("{API}/template/page")).await;
    assert_eq!(global["size"], 0);

    // Only space administrators change it. Bodies may also arrive as ADF.
    let update = json!({
        "templateId": id,
        "name": "On-call runbook",
        "templateType": "page",
        "body": { "atlas_doc_format": { "value": "{\"type\":\"doc\",\"version\":1,\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Steps\"}]}]}", "representation": "atlas_doc_format" } },
    });
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("{API}/template"),
            Some(&dev),
            Some(update.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, updated) = app.put(&format!("{API}/template"), update).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "On-call runbook");
    assert_eq!(updated["body"]["storage"]["value"], "<p>Steps</p>");

    // Global templates are for site administrators.
    let (status, _) = app
        .request(
            Method::POST,
            &format!("{API}/template"),
            Some(&dev),
            Some(json!({ "name": "Mine", "templateType": "page", "body": { "storage": { "value": "<p>x</p>" } } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The editor loads a template as ADF through body conversion.
    let (status, converted) = app
        .post(
            &format!("{API}/contentbody/convert/atlas_doc_format"),
            json!({ "value": "<p>Steps</p>", "representation": "storage" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{converted}");
    assert!(converted["value"].as_str().unwrap().contains("\"Steps\""));

    let (status, _) = app.delete(&format!("{API}/template/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&format!("{API}/template/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
