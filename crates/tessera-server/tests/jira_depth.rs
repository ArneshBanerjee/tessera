//! Components, versions, watchers, votes, links, worklogs, and attachments.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn components_and_automatic_assignee() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (_, bob) = app.user("bob@tessera.test", "Bob").await;

    let (status, ui) = app
        .post(
            "/rest/api/3/component",
            json!({ "project": "PROJ", "name": "UI", "leadAccountId": bob, "assigneeType": "COMPONENT_LEAD" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ui}");
    assert_eq!(ui["realAssigneeType"], "COMPONENT_LEAD");
    assert_eq!(ui["realAssignee"]["displayName"], "Bob");
    let (status, _) = app
        .post(
            "/rest/api/3/component",
            json!({ "project": "PROJ", "name": "ui" }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "names are unique per project"
    );
    app.post(
        "/rest/api/3/component",
        json!({ "project": "PROJ", "name": "API" }),
    )
    .await;

    // No assignee given: the component lead gets the issue.
    let key = app
        .issue(
            "PROJ",
            "Button broken",
            json!({ "components": [{ "name": "UI" }] }),
        )
        .await;
    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["assignee"]["displayName"], "Bob");
    assert_eq!(issue["fields"]["components"][0]["name"], "UI");

    // An explicit null keeps it unassigned.
    let key2 = app
        .issue(
            "PROJ",
            "Unassigned",
            json!({ "components": [{ "name": "UI" }], "assignee": null }),
        )
        .await;
    let (_, issue2) = app.get(&format!("/rest/api/3/issue/{key2}")).await;
    assert!(issue2["fields"]["assignee"].is_null());

    let (status, _) = app
        .put(
            &format!("/rest/api/3/issue/{key}"),
            json!({ "update": { "components": [{ "add": { "name": "API" } }] } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app
        .get(&format!("/rest/api/3/issue/{key}?expand=changelog"))
        .await;
    assert_eq!(issue["fields"]["components"].as_array().unwrap().len(), 2);
    let history = issue["changelog"]["histories"].to_string();
    assert!(history.contains("\"field\":\"Component\""), "{history}");

    assert_eq!(app.jql("component = UI").await.len(), 2);
    assert_eq!(app.jql("component = API").await, [key.as_str()]);
    assert!(app.jql("component is EMPTY").await.is_empty());

    let (_, list) = app.get("/rest/api/3/project/PROJ/components").await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let ui_id = ui["id"].as_str().unwrap();
    let (_, count) = app
        .get(&format!("/rest/api/3/component/{ui_id}/relatedIssueCounts"))
        .await;
    assert_eq!(count["issueCount"], 2);

    let (_, bad) = app
        .post(
            "/rest/api/3/issue",
            json!({ "fields": {
        "project": { "key": "PROJ" }, "issuetype": { "name": "Task" }, "summary": "x",
        "components": [{ "name": "Nope" }] } }),
        )
        .await;
    assert_eq!(
        bad["errors"]["components"],
        "Component name 'Nope' is not valid"
    );
}

#[tokio::test]
async fn versions_and_version_functions() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let create = |name: &'static str, released: bool| {
        app.post("/rest/api/3/version", json!({ "project": "PROJ", "name": name, "released": released, "releaseDate": "2026-01-01" }))
    };
    let (status, v1) = create("1.0", true).await;
    assert_eq!(status, StatusCode::CREATED, "{v1}");
    assert_eq!(v1["userReleaseDate"], "01/Jan/26");
    create("2.0", false).await;
    create("3.0", false).await;

    let key = app
        .issue(
            "PROJ",
            "Fix it",
            json!({ "fixVersions": [{ "name": "2.0" }], "versions": [{ "name": "1.0" }] }),
        )
        .await;
    let older = app
        .issue(
            "PROJ",
            "Old fix",
            json!({ "fixVersions": [{ "name": "1.0" }] }),
        )
        .await;

    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["fixVersions"][0]["name"], "2.0");
    assert_eq!(issue["fields"]["versions"][0]["name"], "1.0");
    assert_eq!(
        issue["fields"]["fixVersions"][0]["overdue"], true,
        "unreleased and past its date"
    );

    assert_eq!(app.jql("fixVersion = 2.0").await, [key.as_str()]);
    assert_eq!(app.jql("affectedVersion = 1.0").await, [key.as_str()]);
    assert_eq!(
        app.jql("fixVersion in releasedVersions(PROJ)").await,
        [older.as_str()]
    );
    assert_eq!(
        app.jql("fixVersion in unreleasedVersions()").await,
        [key.as_str()]
    );
    assert_eq!(
        app.jql("fixVersion = earliestUnreleasedVersion(PROJ)")
            .await,
        [key.as_str()]
    );
    assert_eq!(
        app.jql("fixVersion = latestReleasedVersion(PROJ)").await,
        [older.as_str()]
    );
    assert_eq!(app.jql("fixVersion > 1.0").await, [key.as_str()]);

    let (_, counts) = app
        .get(&format!(
            "/rest/api/3/version/{}/relatedIssueCounts",
            v1["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(counts["issuesFixedCount"], 1);
    assert_eq!(counts["issuesAffectedCount"], 1);

    // Moving 3.0 first changes the order.
    let (_, versions) = app.get("/rest/api/3/project/PROJ/versions").await;
    let v3 = versions[2]["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .post(
            &format!("/rest/api/3/version/{v3}/move"),
            json!({ "position": "First" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, versions) = app.get("/rest/api/3/project/PROJ/versions").await;
    assert_eq!(versions[0]["name"], "3.0");

    // Deleting a version can move its issues to another one.
    let v2 = issue["fields"]["fixVersions"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = app
        .delete(&format!("/rest/api/3/version/{v2}?moveFixIssuesTo={v3}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(app.jql("fixVersion = 3.0").await, [key]);
}

#[tokio::test]
async fn watchers_and_votes() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (bob, bob_id) = app.user("bob@tessera.test", "Bob").await;
    let key = app.issue("PROJ", "Watch me", json!({})).await;

    let (_, watchers) = app.get(&format!("/rest/api/3/issue/{key}/watchers")).await;
    assert_eq!(watchers["watchCount"], 1, "the creator watches");
    assert_eq!(watchers["isWatching"], true);

    // Bob may add himself but not remove the admin.
    let (status, _) = app
        .request(
            Method::POST,
            &format!("/rest/api/3/issue/{key}/watchers"),
            Some(&bob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, admin) = app.get("/rest/api/3/myself").await;
    let admin_id = admin["accountId"].as_str().unwrap();
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/rest/api/3/issue/{key}/watchers?accountId={admin_id}"),
            Some(&bob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The reporter cannot vote; Bob can.
    let (status, err) = app
        .post(&format!("/rest/api/3/issue/{key}/votes"), json!(null))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["errorMessages"][0]
            .as_str()
            .unwrap()
            .contains("reported")
    );
    let (status, _) = app
        .request(
            Method::POST,
            &format!("/rest/api/3/issue/{key}/votes"),
            Some(&bob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, votes) = app
        .request(
            Method::GET,
            &format!("/rest/api/3/issue/{key}/votes"),
            Some(&bob),
            None,
        )
        .await;
    assert_eq!(votes["votes"], 1);
    assert_eq!(votes["hasVoted"], true);
    assert_eq!(votes["voters"][0]["accountId"], bob_id.as_str());

    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["watches"]["watchCount"], 2);
    assert_eq!(issue["fields"]["votes"]["votes"], 1);

    assert_eq!(app.jql("watcher = bob@tessera.test").await, [key.as_str()]);
    assert_eq!(app.jql("voter = bob@tessera.test").await, [key.as_str()]);
    assert_eq!(app.jql("votes > 0").await, [key.as_str()]);
    assert_eq!(app.jql("watchers >= 2").await, [key.as_str()]);
    assert_eq!(app.jql("issue in watchedIssues()").await, [key.as_str()]);
    assert!(
        app.jql("issue in votedIssues()").await.is_empty(),
        "the admin did not vote"
    );
    assert_eq!(app.jql("ORDER BY votes DESC").await[0], key);
}

#[tokio::test]
async fn issue_links() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let a = app.issue("PROJ", "Blocker", json!({})).await;
    let b = app.issue("PROJ", "Blocked", json!({})).await;
    let c = app.issue("PROJ", "Related", json!({})).await;

    // "A blocks B": A is the outward (from) issue.
    let (status, _) = app
        .post(
            "/rest/api/3/issueLink",
            json!({ "type": { "name": "Blocks" }, "outwardIssue": { "key": a }, "inwardIssue": { "key": b },
                    "comment": { "body": "Blocking for now" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    app.post("/rest/api/3/issueLink", json!({ "type": { "name": "Relates" }, "outwardIssue": { "key": a }, "inwardIssue": { "key": c } }))
        .await;

    let (_, issue_a) = app.get(&format!("/rest/api/3/issue/{a}")).await;
    let links_a = issue_a["fields"]["issuelinks"].as_array().unwrap();
    let blocks = links_a
        .iter()
        .find(|l| l["type"]["name"] == "Blocks")
        .unwrap();
    assert_eq!(
        blocks["outwardIssue"]["key"],
        b.as_str(),
        "A shows: blocks B"
    );
    assert_eq!(
        issue_a["fields"]["comment"]["total"], 1,
        "the comment goes on the outward issue"
    );

    let (_, issue_b) = app.get(&format!("/rest/api/3/issue/{b}")).await;
    assert_eq!(
        issue_b["fields"]["issuelinks"][0]["inwardIssue"]["key"],
        a.as_str(),
        "B shows: is blocked by A"
    );

    assert_eq!(
        app.jql(&format!("issue in linkedIssues({a}, \"blocks\")"))
            .await,
        [b.as_str()]
    );
    assert_eq!(
        app.jql(&format!("issue in linkedIssues({b}, \"is blocked by\")"))
            .await,
        [a.as_str()]
    );
    let mut all_linked = app.jql(&format!("issue in linkedIssues({a})")).await;
    all_linked.sort();
    assert_eq!(all_linked, [b.clone(), c.clone()]);
    assert_eq!(app.jql("issueLinkType = blocks").await, [a.as_str()]);
    assert_eq!(
        app.jql(r#"issueLinkType = "is blocked by""#).await,
        [b.as_str()]
    );

    let link_id = blocks["id"].as_str().unwrap();
    let (_, link) = app.get(&format!("/rest/api/3/issueLink/{link_id}")).await;
    assert_eq!(link["outwardIssue"]["key"], a.as_str());
    assert_eq!(link["inwardIssue"]["key"], b.as_str());
    let (status, _) = app
        .delete(&format!("/rest/api/3/issueLink/{link_id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(app.jql("issueLinkType = blocks").await.is_empty());

    let (_, types) = app.get("/rest/api/3/issueLinkType").await;
    assert_eq!(types["issueLinkTypes"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn worklogs_and_time_tracking() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let key = app
        .issue(
            "PROJ",
            "Estimate me",
            json!({ "timetracking": { "originalEstimate": "1d" } }),
        )
        .await;

    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["timetracking"]["originalEstimate"], "1d");
    assert_eq!(
        issue["fields"]["timetracking"]["remainingEstimate"], "1d",
        "set from the original estimate"
    );

    let (status, worklog) = app
        .post(
            &format!("/rest/api/3/issue/{key}/worklog"),
            json!({ "timeSpent": "3h", "started": "2026-10-13T09:00:00.000+0000", "comment": "Did a part" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{worklog}");
    assert_eq!(worklog["timeSpentSeconds"], 10800);
    assert_eq!(worklog["comment"]["type"], "doc");

    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    let fields = &issue["fields"];
    assert_eq!(fields["timespent"], 10800);
    assert_eq!(
        fields["timetracking"]["remainingEstimate"], "5h",
        "auto adjust: 8h - 3h"
    );
    assert_eq!(fields["workratio"], 37);
    assert_eq!(fields["worklog"]["total"], 1);
    assert_eq!(fields["progress"]["percent"], 37);

    // adjustEstimate=new sets the remaining estimate.
    app.post(
        &format!("/rest/api/3/issue/{key}/worklog?adjustEstimate=new&newEstimate=2h"),
        json!({ "timeSpent": "1h" }),
    )
    .await;
    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["timeestimate"], 7200);

    // Deleting a worklog with auto adds the time back.
    let id = worklog["id"].as_str().unwrap();
    let (status, _) = app
        .delete(&format!("/rest/api/3/issue/{key}/worklog/{id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app
        .get(&format!("/rest/api/3/issue/{key}?expand=changelog"))
        .await;
    assert_eq!(issue["fields"]["timeestimate"], 7200 + 10800);
    assert!(
        issue["changelog"]["histories"]
            .to_string()
            .contains("timespent")
    );

    assert_eq!(app.jql("timespent = 1h").await, [key.as_str()]);
    assert_eq!(app.jql("originalEstimate = 1d").await, [key.as_str()]);
    assert_eq!(app.jql("remainingEstimate > 4h").await, [key.as_str()]);
    assert_eq!(
        app.jql("worklogAuthor = currentUser()").await,
        [key.as_str()]
    );
    assert_eq!(app.jql("worklogDate >= -1d").await, [key.as_str()]);

    let (status, err) = app
        .post(
            &format!("/rest/api/3/issue/{key}/worklog"),
            json!({ "timeSpent": "lots" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
}

#[tokio::test]
async fn attachments() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let key = app.issue("PROJ", "With a file", json!({})).await;
    let boundary = "tessera-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\n\
         Content-Type: text/plain\r\n\r\nhello attachment\r\n--{boundary}--\r\n"
    )
    .into_bytes();
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let path = format!("/rest/api/3/issue/{key}/attachments");

    // Without the XSRF header, Jira refuses the upload.
    let (status, _, _) = app
        .raw(
            Method::POST,
            &path,
            &[("content-type", &content_type)],
            body.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, bytes) = app
        .raw(
            Method::POST,
            &path,
            &[
                ("content-type", &content_type),
                ("X-Atlassian-Token", "no-check"),
            ],
            body,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let uploaded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(uploaded[0]["filename"], "notes.txt");
    assert_eq!(uploaded[0]["size"], 16);
    let id = uploaded[0]["id"].as_str().unwrap().to_string();

    let (_, headers, content) = app
        .raw(
            Method::GET,
            &format!("/rest/api/3/attachment/content/{id}"),
            &[],
            vec![],
        )
        .await;
    assert_eq!(content, b"hello attachment");
    assert_eq!(headers["content-type"], "text/plain");

    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"]["attachment"][0]["id"], id.as_str());
    assert_eq!(app.jql("attachments IS NOT EMPTY").await, [key.as_str()]);

    let (status, _) = app.delete(&format!("/rest/api/3/attachment/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&format!("/rest/api/3/attachment/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
