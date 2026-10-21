//! Issue security: schemes, levels and their members, and who sees secured issues.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/rest/api/3";

/// Issue keys a user finds with JQL, sorted.
async fn found(app: &TestApp, auth: &str, jql: &str) -> Vec<String> {
    let (status, body) = app
        .request(
            Method::POST,
            &format!("{API}/search"),
            Some(auth),
            Some(json!({ "jql": jql, "fields": ["summary"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut keys: Vec<String> = body["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap().to_string())
        .collect();
    keys.sort();
    keys
}

/// The id of a level in a scheme, by name.
fn level_id(scheme: &Value, name: &str) -> String {
    scheme["levels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn security_levels_decide_who_sees_an_issue() {
    let app = TestApp::new().await;
    let (dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    app.post(&format!("{API}/group"), json!({ "name": "staff" }))
        .await;
    let project = app.project("SEC").await;
    let (_, info) = app.get(&format!("{API}/project/SEC")).await;
    let project_id = info["id"].as_str().unwrap().to_string();

    // A scheme with two levels. Staff is the default; its members are the staff group and
    // each issue's reporter.
    let (status, created) = app
        .post(
            &format!("{API}/issuesecurityschemes"),
            json!({
                "name": "Confidential",
                "description": "Who sees what",
                "levels": [
                    { "name": "Staff", "isDefault": true, "members": [
                        { "type": "group", "parameter": "staff" },
                        { "type": "reporter" },
                    ] },
                    { "name": "Secret", "members": [{ "type": "projectrole", "parameter": "10002" }] },
                ],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let scheme_id = created["id"].as_str().unwrap().to_string();
    let (_, scheme) = app
        .get(&format!("{API}/issuesecurityschemes/{scheme_id}"))
        .await;
    let staff = level_id(&scheme, "Staff");
    let secret = level_id(&scheme, "Secret");
    assert_eq!(scheme["defaultSecurityLevelId"].to_string(), staff);
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/project"),
            json!({ "schemeId": scheme_id, "projectId": project_id }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Issues get the default level unless the request names one, or none.
    let defaulted = app.issue(&project, "Defaulted", json!({})).await;
    let open = app
        .issue(&project, "Open", json!({ "security": null }))
        .await;
    let hidden = app
        .issue(
            &project,
            "Hidden",
            json!({ "security": { "name": "Secret" } }),
        )
        .await;
    let (_, issue) = app
        .get(&format!("{API}/issue/{defaulted}?fields=security"))
        .await;
    assert_eq!(issue["fields"]["security"]["name"], "Staff");
    let (_, issue) = app
        .get(&format!("{API}/issue/{open}?fields=security"))
        .await;
    assert_eq!(issue["fields"]["security"], Value::Null);

    // Dev, outside the staff group, sees only the open issue.
    assert_eq!(
        found(&app, &dev, "project = SEC").await,
        std::slice::from_ref(&open)
    );
    let (status, _) = app
        .request(
            Method::GET,
            &format!("{API}/issue/{defaulted}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Dev's own issue gets the default level too, and Dev sees it as its reporter.
    let (status, mine) = app
        .request(
            Method::POST,
            &format!("{API}/issue"),
            Some(&dev),
            Some(json!({ "fields": { "project": { "key": "SEC" }, "issuetype": { "name": "Task" }, "summary": "Mine" } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    let mine = mine["key"].as_str().unwrap().to_string();
    let mut expected = vec![mine.clone(), open.clone()];
    expected.sort();
    assert_eq!(found(&app, &dev, "project = SEC").await, expected);

    // As in Jira, a site administrator outside a level does not see its issues either: the
    // administrator is not in the staff group, nor the reporter of Dev's issue.
    let (status, _) = app.get(&format!("{API}/issue/{mine}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !found(&app, &app.admin, "project = SEC")
            .await
            .contains(&mine)
    );
    let (_, me) = app.get(&format!("{API}/myself")).await;
    app.post(
        &format!("{API}/group/user?groupname=staff"),
        json!({ "accountId": me["accountId"] }),
    )
    .await;
    let (status, _) = app.get(&format!("{API}/issue/{mine}")).await;
    assert_eq!(status, StatusCode::OK);

    // Only project administrators may set a level, and only to levels they belong to.
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("{API}/issue/{mine}"),
            Some(&dev),
            Some(json!({ "fields": { "security": null } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, meta) = app
        .request(
            Method::GET,
            &format!("{API}/issue/{mine}/editmeta"),
            Some(&dev),
            None,
        )
        .await;
    assert!(meta["fields"]["security"].is_null(), "{meta}");
    let (_, meta) = app.get(&format!("{API}/issue/{open}/editmeta")).await;
    assert_eq!(
        meta["fields"]["security"]["allowedValues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, levels) = app.get(&format!("{API}/project/SEC/securitylevel")).await;
    assert_eq!(levels["levels"].as_array().unwrap().len(), 2);
    let (_, levels) = app
        .request(
            Method::GET,
            &format!("{API}/project/SEC/securitylevel"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(levels["levels"], json!([]));

    // Links to issues Dev cannot see are left out.
    let (status, _) = app
        .post(
            &format!("{API}/issueLink"),
            json!({ "type": { "name": "Relates" }, "outwardIssue": { "key": open }, "inwardIssue": { "key": hidden } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, viewed) = app
        .request(
            Method::GET,
            &format!("{API}/issue/{open}?fields=issuelinks"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(viewed["fields"]["issuelinks"], json!([]));
    let (_, viewed) = app
        .get(&format!("{API}/issue/{open}?fields=issuelinks"))
        .await;
    assert_eq!(viewed["fields"]["issuelinks"].as_array().unwrap().len(), 1);

    // Joining the staff group shows the Staff issues, but not the Secret one.
    app.post(
        &format!("{API}/group/user?groupname=staff"),
        json!({ "accountId": dev_account }),
    )
    .await;
    let mut expected = vec![defaulted.clone(), mine.clone(), open.clone()];
    expected.sort();
    assert_eq!(found(&app, &dev, "project = SEC").await, expected);

    // JQL finds issues by level.
    assert_eq!(
        found(&app, &app.admin, "level = Secret").await,
        std::slice::from_ref(&hidden)
    );
    assert_eq!(
        found(&app, &app.admin, "level is EMPTY").await,
        std::slice::from_ref(&open)
    );
    let mut staff_issues = vec![defaulted.clone(), mine.clone()];
    staff_issues.sort();
    assert_eq!(found(&app, &app.admin, "level = Staff").await, staff_issues);

    // Subtasks share their parent's level; changes are kept in the history.
    let subtask = app
        .issue(
            &project,
            "Part",
            json!({ "issuetype": { "name": "Subtask" }, "parent": { "key": defaulted } }),
        )
        .await;
    let (_, part) = app
        .get(&format!("{API}/issue/{subtask}?fields=security"))
        .await;
    assert_eq!(part["fields"]["security"]["name"], "Staff");
    let (status, _) = app
        .put(
            &format!("{API}/issue/{defaulted}"),
            json!({ "fields": { "security": { "id": secret } } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, part) = app
        .get(&format!("{API}/issue/{subtask}?fields=security"))
        .await;
    assert_eq!(part["fields"]["security"]["name"], "Secret");
    let (_, history) = app.get(&format!("{API}/issue/{defaulted}/changelog")).await;
    let item = &history["values"][0]["items"][0];
    assert_eq!(item["field"], "security");
    assert_eq!(item["fromString"], "Staff");
    assert_eq!(item["toString"], "Secret");

    // Deleting a level moves its issues to the replacement.
    let (status, _) = app
        .delete(&format!(
            "{API}/issuesecurityschemes/{scheme_id}/level/{secret}?replaceWith={staff}"
        ))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app
        .get(&format!("{API}/issue/{hidden}?fields=security"))
        .await;
    assert_eq!(issue["fields"]["security"]["name"], "Staff");

    // Moving to another scheme needs a new level for each level issues use, as in Jira.
    let (_, other) = app
        .post(
            &format!("{API}/issuesecurityschemes"),
            json!({ "name": "Other", "levels": [{ "name": "Internal", "members": [{ "type": "reporter" }] }] }),
        )
        .await;
    let other_id = other["id"].as_str().unwrap().to_string();
    let (_, other_scheme) = app
        .get(&format!("{API}/issuesecurityschemes/{other_id}"))
        .await;
    let internal = level_id(&other_scheme, "Internal");
    let (status, refused) = app
        .put(
            &format!("{API}/issuesecurityschemes/project"),
            json!({ "schemeId": other_id, "projectId": project_id }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/project"),
            json!({
                "schemeId": other_id,
                "projectId": project_id,
                "oldToNewSecurityLevelMappings": [{ "oldLevelId": staff, "newLevelId": internal }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app
        .get(&format!("{API}/issue/{hidden}?fields=security"))
        .await;
    assert_eq!(issue["fields"]["security"]["name"], "Internal");
    // And back again, this time with the levels dropped.
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/project"),
            json!({
                "schemeId": scheme_id,
                "projectId": project_id,
                "oldToNewSecurityLevelMappings": [{ "oldLevelId": internal, "newLevelId": "-1" }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        found(&app, &app.admin, "level is not EMPTY").await,
        Vec::<String>::new()
    );
    app.put(
        &format!("{API}/issue/{hidden}"),
        json!({ "fields": { "security": { "id": staff } } }),
    )
    .await;

    // A scheme in use cannot be deleted; without it, every issue is open again.
    let (status, _) = app
        .delete(&format!("{API}/issuesecurityschemes/{scheme_id}"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/project"),
            json!({ "schemeId": "-1", "projectId": project_id }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        found(&app, &app.admin, "level is not EMPTY").await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn levels_and_members_are_managed_separately() {
    let app = TestApp::new().await;
    let (_, list) = app.get(&format!("{API}/issuesecurityschemes")).await;
    assert_eq!(
        list["issueSecuritySchemes"][0]["name"],
        "Default issue security scheme"
    );
    let (_, created) = app
        .post(
            &format!("{API}/issuesecurityschemes"),
            json!({ "name": "Levels" }),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/{id}/level"),
            json!({ "levels": [{ "name": "Partners", "description": "Outside teams" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, levels) = app
        .get(&format!("{API}/issuesecurityschemes/level?schemeId={id}"))
        .await;
    assert_eq!(levels["total"], 1);
    let level = levels["values"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(levels["values"][0]["isDefault"], false);

    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/{id}/level/{level}/member"),
            json!({ "members": [{ "type": "reporter" }, { "type": "group", "parameter": "nobody-here" }] }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown groups are refused"
    );
    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/{id}/level/{level}/member"),
            json!({ "members": [{ "type": "reporter" }, { "type": "lead" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, members) = app
        .get(&format!(
            "{API}/issuesecurityschemes/level/member?schemeId={id}&levelId={level}"
        ))
        .await;
    assert_eq!(members["total"], 2);
    assert_eq!(members["values"][1]["holder"]["type"], "projectLead");
    let member = members["values"][0]["id"].as_str().unwrap();
    let (status, _) = app
        .delete(&format!(
            "{API}/issuesecurityschemes/{id}/level/{level}/member/{member}"
        ))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app
        .put(
            &format!("{API}/issuesecurityschemes/level/default"),
            json!({ "defaultValues": [{ "issueSecuritySchemeId": id, "defaultLevelId": level }] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, read) = app.get(&format!("{API}/securitylevel/{level}")).await;
    assert_eq!(read["name"], "Partners");
    let (_, scheme) = app.get(&format!("{API}/issuesecurityschemes/{id}")).await;
    assert_eq!(scheme["defaultSecurityLevelId"].to_string(), level);

    // Only site administrators manage schemes.
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let (status, _) = app
        .request(
            Method::POST,
            &format!("{API}/issuesecurityschemes"),
            Some(&dev),
            Some(json!({ "name": "Mine" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
