//! End-to-end tests of the Jira-compatible REST API.

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, basic};
use serde_json::json;

#[tokio::test]
async fn authentication() {
    let app = TestApp::new().await;
    let (status, body) = app
        .request(Method::GET, "/rest/api/3/myself", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body["errorMessages"][0]
            .as_str()
            .unwrap()
            .contains("not authenticated")
    );

    // An API token and the password both work.
    let (status, me) = app.get("/rest/api/3/myself").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["displayName"], "Ada Admin");
    assert_eq!(me["accountType"], "atlassian");
    let password = basic("admin@tessera.test", "secret");
    let (status, _) = app
        .request(Method::GET, "/rest/api/2/myself", Some(&password), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let wrong = basic("admin@tessera.test", "nope");
    let (status, _) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&wrong), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // serverInfo is public.
    let (status, info) = app
        .request(Method::GET, "/rest/api/3/serverInfo", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["deploymentType"], "Cloud");
}

#[tokio::test]
async fn sessions_for_the_web_app() {
    let app = TestApp::new().await;
    let (status, body) = app
        .request(
            Method::POST,
            "/rest/auth/1/session",
            None,
            Some(json!({ "username": "admin@tessera.test", "password": "secret" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session"]["name"], "tessera.session");
    let (status, _) = app
        .request(
            Method::POST,
            "/rest/auth/1/session",
            None,
            Some(json!({ "username": "admin@tessera.test", "password": "bad" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn metadata_matches_a_new_jira_site() {
    let app = TestApp::new().await;
    let (_, types) = app.get("/rest/api/3/issuetype").await;
    let names: Vec<&str> = types
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Epic", "Story", "Task", "Bug", "Subtask"]);
    assert_eq!(types[4]["subtask"], true);

    let (_, priorities) = app.get("/rest/api/3/priority").await;
    assert_eq!(priorities[0]["name"], "Highest");
    assert_eq!(priorities[0]["id"], "1");

    let (_, status) = app.get("/rest/api/3/status/In%20Progress").await;
    assert_eq!(status["statusCategory"]["key"], "indeterminate");

    let (_, categories) = app.get("/rest/api/3/statuscategory").await;
    assert_eq!(categories.as_array().unwrap().len(), 4);

    let (_, fields) = app.get("/rest/api/3/field").await;
    assert!(
        fields
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "summary" && f["custom"] == false)
    );
}

#[tokio::test]
async fn projects() {
    let app = TestApp::new().await;
    let (status, created) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "PROJ", "name": "Project", "description": "Main" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["key"], "PROJ");
    assert!(created["id"].is_number(), "Jira returns a numeric id here");

    let (status, err) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "PROJ", "name": "Other" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["errors"]["projectKey"]
            .as_str()
            .unwrap()
            .contains("uses this project key")
    );

    let (status, err) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "bad key", "name": "X" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["errors"]["projectKey"].is_string());

    let (_, project) = app.get("/rest/api/3/project/PROJ").await;
    assert_eq!(project["name"], "Project");
    assert_eq!(project["lead"]["displayName"], "Ada Admin");
    assert_eq!(project["issueTypes"].as_array().unwrap().len(), 5);

    let (_, page) = app.get("/rest/api/3/project/search?query=proj").await;
    assert_eq!(page["total"], 1);

    let (status, _) = app
        .put("/rest/api/3/project/PROJ", json!({ "name": "Renamed" }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = app.get("/rest/api/3/project").await;
    assert_eq!(list[0]["name"], "Renamed");

    // Regular users cannot create projects.
    let (bob, _) = app.user("bob@tessera.test", "Bob").await;
    let (status, _) = app
        .request(
            Method::POST,
            "/rest/api/3/project",
            Some(&bob),
            Some(json!({ "key": "BOB", "name": "Bob" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = app.delete("/rest/api/3/project/PROJ").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get("/rest/api/3/project/PROJ").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_and_read_issues() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (_, bob) = app.user("bob@tessera.test", "Bob").await;

    let first = app.issue("PROJ", "First", json!({})).await;
    let second = app
        .issue(
            "PROJ",
            "Second",
            json!({
                "description": { "type": "doc", "version": 1, "content": [
                    { "type": "paragraph", "content": [{ "type": "text", "text": "Rich text" }] }
                ]},
                "priority": { "name": "High" },
                "assignee": { "accountId": bob },
                "labels": ["backend", "api"],
                "duedate": "2026-12-01",
            }),
        )
        .await;
    assert_eq!((first.as_str(), second.as_str()), ("PROJ-1", "PROJ-2"));

    let (_, v3) = app.get("/rest/api/3/issue/PROJ-2").await;
    let fields = &v3["fields"];
    assert_eq!(fields["summary"], "Second");
    assert_eq!(fields["description"]["type"], "doc");
    assert_eq!(fields["status"]["name"], "To Do");
    assert_eq!(fields["priority"]["name"], "High");
    assert_eq!(fields["assignee"]["displayName"], "Bob");
    assert_eq!(fields["reporter"]["displayName"], "Ada Admin");
    assert_eq!(fields["labels"], json!(["api", "backend"]));
    assert_eq!(fields["duedate"], "2026-12-01");
    assert_eq!(fields["project"]["key"], "PROJ");
    assert!(fields["created"].as_str().unwrap().ends_with("+0000"));

    // The v2 API returns rich text as a plain string.
    let (_, v2) = app.get("/rest/api/2/issue/PROJ-2").await;
    assert_eq!(v2["fields"]["description"], "Rich text");

    // Plain strings are accepted as input too.
    let (status, created) = app
        .post(
            "/rest/api/2/issue",
            json!({ "fields": { "project": { "key": "PROJ" }, "issuetype": { "id": "10002" },
                "summary": "From v2", "description": "Line one\nLine two" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, v2) = app
        .get(&format!(
            "/rest/api/2/issue/{}",
            created["key"].as_str().unwrap()
        ))
        .await;
    assert_eq!(v2["fields"]["description"], "Line one\nLine two");

    // The fields parameter limits the output.
    let (_, only) = app
        .get("/rest/api/3/issue/PROJ-1?fields=summary,status")
        .await;
    let keys: Vec<&String> = only["fields"].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 2);

    let (_, names) = app.get("/rest/api/3/issue/PROJ-1?expand=names").await;
    assert_eq!(names["names"]["summary"], "Summary");

    let (status, _) = app.get("/rest/api/3/issue/PROJ-99").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_validation_matches_jira() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let create =
        |fields: serde_json::Value| app.post("/rest/api/3/issue", json!({ "fields": fields }));

    let (status, err) = create(json!({ "issuetype": { "name": "Task" }, "summary": "x" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["errors"]["project"].is_string());

    let (_, err) = create(json!({ "project": { "key": "PROJ" }, "summary": "x" })).await;
    assert_eq!(err["errors"]["issuetype"], "Specify an issue type");

    let (_, err) =
        create(json!({ "project": { "key": "PROJ" }, "issuetype": { "name": "Task" } })).await;
    assert_eq!(
        err["errors"]["summary"],
        "You must specify a summary of the issue."
    );

    let (_, err) = create(
        json!({ "project": { "key": "PROJ" }, "issuetype": { "name": "Task" },
        "summary": "x", "labels": ["has space"] }),
    )
    .await;
    assert!(
        err["errors"]["labels"]
            .as_str()
            .unwrap()
            .contains("contains spaces")
    );

    let (_, err) = create(
        json!({ "project": { "key": "PROJ" }, "issuetype": { "name": "Task" },
        "summary": "x", "status": { "name": "Done" } }),
    )
    .await;
    assert!(
        err["errors"]["status"]
            .as_str()
            .unwrap()
            .contains("cannot be set")
    );

    let (_, err) = create(
        json!({ "project": { "key": "PROJ" }, "issuetype": { "name": "Subtask" }, "summary": "x" }),
    )
    .await;
    assert!(
        err["errors"]["parent"].is_string(),
        "subtasks need a parent: {err}"
    );
}

#[tokio::test]
async fn edit_records_history() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (_, bob) = app.user("bob@tessera.test", "Bob").await;
    app.issue("PROJ", "Old summary", json!({ "labels": ["a"] }))
        .await;

    let (status, _) = app
        .put(
            "/rest/api/3/issue/PROJ-1",
            json!({
                "fields": { "summary": "New summary", "priority": { "name": "Low" } },
                "update": { "labels": [{ "add": "b" }, { "remove": "a" }] },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .put(
            "/rest/api/3/issue/PROJ-1/assignee",
            json!({ "accountId": bob }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, issue) = app.get("/rest/api/3/issue/PROJ-1?expand=changelog").await;
    assert_eq!(issue["fields"]["summary"], "New summary");
    assert_eq!(issue["fields"]["labels"], json!(["b"]));
    assert_eq!(issue["fields"]["assignee"]["displayName"], "Bob");

    let (_, history) = app.get("/rest/api/3/issue/PROJ-1/changelog").await;
    let items: Vec<(String, String)> = history["values"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["items"].as_array().unwrap().clone())
        .map(|i| {
            (
                i["field"].as_str().unwrap().into(),
                i["toString"].as_str().unwrap_or("").into(),
            )
        })
        .collect();
    assert!(
        items.contains(&("summary".into(), "New summary".into())),
        "{items:?}"
    );
    assert!(items.contains(&("priority".into(), "Low".into())));
    assert!(items.contains(&("labels".into(), "b".into())));
    assert!(items.contains(&("assignee".into(), "Bob".into())));
    assert_eq!(issue["changelog"]["total"], history["total"]);
}

#[tokio::test]
async fn transitions_follow_the_simplified_workflow() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    app.issue("PROJ", "Work", json!({})).await;

    let (_, list) = app.get("/rest/api/3/issue/PROJ-1/transitions").await;
    let ids: Vec<&str> = list["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["21", "31"], "the current status is not offered");

    let (status, _) = app
        .post(
            "/rest/api/3/issue/PROJ-1/transitions",
            json!({ "transition": { "id": "31" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, done) = app.get("/rest/api/3/issue/PROJ-1").await;
    assert_eq!(done["fields"]["status"]["name"], "Done");
    assert_eq!(done["fields"]["resolution"]["name"], "Done");
    assert!(done["fields"]["resolutiondate"].is_string());

    // Reopening clears the resolution. Fields in the request are applied too.
    let (status, _) = app
        .post(
            "/rest/api/3/issue/PROJ-1/transitions",
            json!({ "transition": { "id": "11" }, "fields": { "summary": "Reopened" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, open) = app.get("/rest/api/3/issue/PROJ-1").await;
    assert_eq!(open["fields"]["status"]["name"], "To Do");
    assert!(open["fields"]["resolution"].is_null());
    assert_eq!(open["fields"]["summary"], "Reopened");

    let (status, _) = app
        .post(
            "/rest/api/3/issue/PROJ-1/transitions",
            json!({ "transition": { "id": "11" } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "already in To Do");
}

#[tokio::test]
async fn comments() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    app.issue("PROJ", "Talk", json!({})).await;

    let (status, comment) = app
        .post(
            "/rest/api/2/issue/PROJ-1/comment",
            json!({ "body": "Hello" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(comment["body"], "Hello");
    let id = comment["id"].as_str().unwrap().to_string();

    let (_, v3) = app
        .get(&format!("/rest/api/3/issue/PROJ-1/comment/{id}"))
        .await;
    assert_eq!(v3["body"]["type"], "doc");

    let (status, _) = app
        .put(
            &format!("/rest/api/2/issue/PROJ-1/comment/{id}"),
            json!({ "body": "Edited" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Someone else may not edit it.
    let (bob, _) = app.user("bob@tessera.test", "Bob").await;
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("/rest/api/2/issue/PROJ-1/comment/{id}"),
            Some(&bob),
            Some(json!({ "body": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, list) = app.get("/rest/api/2/issue/PROJ-1/comment").await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["comments"][0]["body"], "Edited");

    let (status, _) = app
        .delete(&format!("/rest/api/2/issue/PROJ-1/comment/{id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn hierarchy_and_deletion() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let epic = app
        .issue("PROJ", "Epic", json!({ "issuetype": { "name": "Epic" } }))
        .await;
    let story = app
        .issue(
            "PROJ",
            "Story",
            json!({ "issuetype": { "name": "Story" }, "parent": { "key": epic } }),
        )
        .await;
    let subtask = app
        .issue(
            "PROJ",
            "Sub",
            json!({ "issuetype": { "name": "Subtask" }, "parent": { "key": story } }),
        )
        .await;

    let (_, parent) = app.get(&format!("/rest/api/3/issue/{story}")).await;
    assert_eq!(parent["fields"]["parent"]["key"], epic.as_str());
    assert_eq!(parent["fields"]["subtasks"][0]["key"], subtask.as_str());

    // A story cannot be the parent of an epic.
    let (status, _) = app
        .put(
            &format!("/rest/api/3/issue/{epic}"),
            json!({ "fields": { "parent": { "key": story } } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = app.delete(&format!("/rest/api/3/issue/{story}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "has subtasks");
    let (status, _) = app
        .delete(&format!("/rest/api/3/issue/{story}?deleteSubtasks=true"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&format!("/rest/api/3/issue/{subtask}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app.get(&format!("/rest/api/3/issue/{epic}")).await;
    assert_eq!(status, StatusCode::OK, "deleting a child keeps the epic");
}

#[tokio::test]
async fn custom_fields() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (status, points) = app
        .post(
            "/rest/api/3/field",
            json!({ "name": "Story Points", "type": "com.atlassian.jira.plugin.system.customfieldtypes:float" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let points_key = points["id"].as_str().unwrap().to_string();
    assert!(points_key.starts_with("customfield_"));

    let (_, team) = app
        .post(
            "/rest/api/3/field",
            json!({ "name": "Team", "type": "com.atlassian.jira.plugin.system.customfieldtypes:select" }),
        )
        .await;
    let team_key = team["id"].as_str().unwrap().to_string();
    let team_id = team["schema"]["customId"].as_i64().unwrap();
    let (_, added) = app
        .post(
            &format!("/rest/api/3/field/{team_key}/context/{team_id}/option"),
            json!({ "options": [{ "value": "Red" }, { "value": "Blue" }] }),
        )
        .await;
    assert_eq!(added["options"].as_array().unwrap().len(), 2);

    let key = app
        .issue(
            "PROJ",
            "Estimated",
            json!({ points_key.clone(): 5, team_key.clone(): { "value": "Blue" } }),
        )
        .await;
    let (_, issue) = app.get(&format!("/rest/api/3/issue/{key}")).await;
    assert_eq!(issue["fields"][&points_key], 5.0);
    assert_eq!(issue["fields"][&team_key]["value"], "Blue");

    let (status, err) = app
        .put(
            &format!("/rest/api/3/issue/{key}"),
            json!({ "fields": { team_key.clone(): { "value": "Green" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");

    let (_, meta) = app.get(&format!("/rest/api/3/issue/{key}/editmeta")).await;
    assert_eq!(
        meta["fields"][&team_key]["allowedValues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn create_metadata() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (_, meta) = app
        .get("/rest/api/2/issue/createmeta?projectKeys=PROJ&issuetypeNames=Bug&expand=projects.issuetypes.fields")
        .await;
    let issue_types = meta["projects"][0]["issuetypes"].as_array().unwrap();
    assert_eq!(issue_types.len(), 1);
    assert_eq!(issue_types[0]["fields"]["summary"]["required"], true);

    let (_, types) = app
        .get("/rest/api/3/issue/createmeta/PROJ/issuetypes")
        .await;
    assert_eq!(types["total"], 5);
    let (_, fields) = app
        .get("/rest/api/3/issue/createmeta/PROJ/issuetypes/10002")
        .await;
    assert!(
        fields["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["fieldId"] == "priority")
    );
}

#[tokio::test]
async fn schemes_and_project_categories() {
    let app = TestApp::new().await;
    let (_, schemes) = app.get("/rest/api/2/permissionscheme").await;
    assert_eq!(
        schemes["permissionSchemes"][0]["name"],
        "Default Permission Scheme"
    );
    let (_, security) = app.get("/rest/api/2/issuesecurityschemes").await;
    assert_eq!(security["issueSecuritySchemes"][0]["name"], "Default");

    let (status, category) = app
        .post(
            "/rest/api/3/projectCategory",
            json!({ "name": "Internal", "description": "Our tools" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let category_id: i64 = category["id"].as_str().unwrap().parse().unwrap();
    let (status, _) = app
        .post("/rest/api/3/projectCategory", json!({ "name": "internal" }))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "names are unique, ignoring case"
    );

    let (status, _) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "TOOL", "name": "Tools", "categoryId": category_id,
                    "assigneeType": "PROJECT_LEAD", "url": "https://example.com",
                    "permissionScheme": 0, "notificationScheme": 10000 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, project) = app.get("/rest/api/3/project/TOOL").await;
    assert_eq!(project["projectCategory"]["name"], "Internal");
    assert_eq!(project["assigneeType"], "PROJECT_LEAD");
    assert_eq!(project["url"], "https://example.com");

    let (status, _) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "BAD", "name": "Bad", "categoryId": 999 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = app
        .delete(&format!("/rest/api/3/projectCategory/{category_id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, project) = app.get("/rest/api/3/project/TOOL").await;
    assert!(
        project.get("projectCategory").is_none(),
        "the project keeps existing without a category"
    );
}
