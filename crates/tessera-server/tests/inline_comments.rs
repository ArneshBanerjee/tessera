//! Confluence inline comments: comments on a piece of a page's text.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const V1: &str = "/wiki/rest/api";
const V2: &str = "/wiki/api/v2";

async fn page(app: &TestApp, storage: &str) -> String {
    app.post(
        &format!("{V1}/space"),
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let (status, page) = app
        .post(
            &format!("{V1}/content"),
            json!({ "type": "page", "title": "Plan", "space": { "key": "DOCS" },
                    "body": { "storage": { "value": storage, "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["id"].as_str().unwrap().to_string()
}

fn text_body(text: &str) -> Value {
    json!({ "representation": "storage", "value": format!("<p>{text}</p>") })
}

async fn storage_of(app: &TestApp, id: &str) -> String {
    let (_, found) = app
        .get(&format!("{V1}/content/{id}?expand=body.storage,version"))
        .await;
    found["body"]["storage"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn inline_comments_mark_their_text() {
    let app = TestApp::new().await;
    let id = page(
        &app,
        "<p>Ship the build. Then ship the docs &amp; notes.</p>",
    )
    .await;

    // Matching is exact, so "ship the docs & notes" occurs once: index 0.
    let (status, comment) = app
        .post(
            &format!("{V2}/inline-comments"),
            json!({ "pageId": id, "body": text_body("Which docs?"),
                    "inlineCommentProperties": { "textSelection": "ship the docs & notes", "textSelectionMatchCount": 1, "textSelectionMatchIndex": 0 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{comment}");
    assert_eq!(comment["resolutionStatus"], "open");
    let reference = comment["properties"]["inlineMarkerRef"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        storage_of(&app, &id).await,
        format!(
            "<p>Ship the build. Then <ac:inline-comment-marker ac:ref=\"{reference}\">ship the docs &amp; notes</ac:inline-comment-marker>.</p>"
        )
    );
    // Marking text makes no new version.
    let (_, versions) = app.get(&format!("{V2}/pages/{id}/versions")).await;
    assert_eq!(versions["results"].as_array().unwrap().len(), 1);

    // The view shows the marker, and v1 lists the comment as inline.
    let (_, view) = app
        .get(&format!("{V1}/content/{id}?expand=body.view"))
        .await;
    assert!(
        view["body"]["view"]["value"]
            .as_str()
            .unwrap()
            .contains(&format!("data-ref=\"{reference}\""))
    );
    let (_, inline) = app
        .get(&format!(
            "{V1}/content/{id}/child/comment?location=inline&expand=extensions.inlineProperties"
        ))
        .await;
    assert_eq!(inline["size"], 1);
    assert_eq!(inline["results"][0]["extensions"]["location"], "inline");
    assert_eq!(
        inline["results"][0]["extensions"]["inlineProperties"]["originalSelection"],
        "ship the docs & notes"
    );
    let (_, footer) = app
        .get(&format!("{V1}/content/{id}/child/comment?location=footer"))
        .await;
    assert_eq!(footer["size"], 0);
    let (_, footers) = app.get(&format!("{V2}/pages/{id}/footer-comments")).await;
    assert_eq!(footers["results"], json!([]));

    // A reply is inline on the same text.
    let comment_id = comment["id"].as_str().unwrap();
    let (status, reply) = app
        .post(
            &format!("{V2}/inline-comments"),
            json!({ "parentCommentId": comment_id, "body": text_body("The release notes.") }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["properties"]["inlineMarkerRef"], json!(reference));
    let (_, replies) = app
        .get(&format!("{V2}/inline-comments/{comment_id}/children"))
        .await;
    assert_eq!(replies["results"].as_array().unwrap().len(), 1);

    // Resolving and reopening.
    let (status, resolved) = app
        .put(
            &format!("{V2}/inline-comments/{comment_id}"),
            json!({ "resolved": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["resolutionStatus"], "resolved");
    let (_, open) = app
        .get(&format!(
            "{V2}/pages/{id}/inline-comments?resolution-status=open"
        ))
        .await;
    assert_eq!(open["results"], json!([]));
    let (_, reopened) = app
        .put(
            &format!("{V2}/inline-comments/{comment_id}"),
            json!({ "resolved": false }),
        )
        .await;
    assert_eq!(reopened["resolutionStatus"], "reopened");

    // Text not on the page is refused.
    let (status, _) = app
        .post(
            &format!("{V2}/inline-comments"),
            json!({ "pageId": id, "body": text_body("?"), "inlineCommentProperties": { "textSelection": "nowhere" } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // When an edit removes the marked text, the comment is dangling.
    let (status, _) = app
        .put(
            &format!("{V1}/content/{id}"),
            json!({ "type": "page", "title": "Plan", "version": { "number": 2 },
                    "body": { "storage": { "value": "<p>Rewritten.</p>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, found) = app.get(&format!("{V2}/inline-comments/{comment_id}")).await;
    assert_eq!(found["resolutionStatus"], "dangling");
}

#[tokio::test]
async fn markers_skip_settings_and_code() {
    let app = TestApp::new().await;
    let id = page(
        &app,
        "<ac:structured-macro ac:name=\"code\"><ac:parameter ac:name=\"title\">alpha</ac:parameter><ac:plain-text-body><![CDATA[alpha]]></ac:plain-text-body></ac:structured-macro><p>alpha</p>",
    )
    .await;
    let (status, comment) = app
        .post(
            &format!("{V2}/inline-comments"),
            json!({ "pageId": id, "body": text_body("Here"), "inlineCommentProperties": { "textSelection": "alpha" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{comment}");
    let reference = comment["properties"]["inlineMarkerRef"].as_str().unwrap();
    assert!(storage_of(&app, &id).await.ends_with(&format!(
        "<p><ac:inline-comment-marker ac:ref=\"{reference}\">alpha</ac:inline-comment-marker></p>"
    )));
}
