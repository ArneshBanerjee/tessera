//! Dashboards: the system dashboard, sharing, copies, gadgets, and gadget item properties.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/rest/api/3";
const FILTER_RESULTS: &str = "com.atlassian.jira.gadgets:filter-results-dashboard-item";
const PIE_CHART: &str = "com.atlassian.jira.gadgets:pie-chart-dashboard-item";

/// `(id, column, row)` of each gadget, in column and row order.
fn places(list: &Value) -> Vec<(i64, i64, i64)> {
    list["gadgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g["id"].as_i64().unwrap(),
                g["position"]["column"].as_i64().unwrap(),
                g["position"]["row"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_system_dashboard_is_there_for_everyone() {
    let app = TestApp::new().await;
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let (status, list) = app
        .request(Method::GET, &format!("{API}/dashboard"), Some(&dev), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 1);
    let system = &list["dashboards"][0];
    assert_eq!(system["id"], "10000");
    assert_eq!(system["name"], "Default dashboard");
    assert_eq!(system["systemDashboard"], true);
    assert_eq!(system["isWritable"], false);
    let (_, gadgets) = app.get(&format!("{API}/dashboard/10000/gadget")).await;
    assert_eq!(gadgets["gadgets"].as_array().unwrap().len(), 2);

    // Only administrators change it, and nobody deletes it.
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("{API}/dashboard/10000"),
            Some(&dev),
            Some(json!({ "name": "Mine now" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app.delete(&format!("{API}/dashboard/10000")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dashboards_are_private_until_shared() {
    let app = TestApp::new().await;
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let (status, created) = app
        .post(
            &format!("{API}/dashboard"),
            json!({ "name": "Team health", "description": "Our work", "sharePermissions": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["isWritable"], true);
    assert_eq!(created["owner"]["displayName"], "Ada Admin");

    let path = format!("{API}/dashboard/{id}");
    let (status, _) = app.request(Method::GET, &path, Some(&dev), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Shared with every signed-in user, Dev sees it but cannot change it.
    let (status, updated) = app
        .put(
            &path,
            json!({ "name": "Team health", "sharePermissions": [{ "type": "loggedin" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["sharePermissions"][0]["type"], "loggedin");
    let (status, seen) = app.request(Method::GET, &path, Some(&dev), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["isWritable"], false);
    let (status, _) = app
        .request(
            Method::POST,
            &format!("{path}/gadget"),
            Some(&dev),
            Some(json!({ "moduleKey": PIE_CHART })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Search by name and owner, and Dev's own list stays empty.
    let (_, found) = app
        .request(
            Method::GET,
            &format!("{API}/dashboard/search?dashboardName=health"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(found["total"], 1);
    assert_eq!(found["values"][0]["name"], "Team health");
    let (_, mine) = app
        .request(
            Method::GET,
            &format!("{API}/dashboard?filter=my"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(mine["total"], 0);

    // Starring puts it in the favourites list.
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("/rest/tessera/1.0/dashboard/{id}/favourite"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, starred) = app
        .request(
            Method::GET,
            &format!("{API}/dashboard?filter=favourite"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(starred["dashboards"][0]["isFavourite"], true);
    assert_eq!(starred["dashboards"][0]["popularity"], 1);

    let (status, _) = app.delete(&path).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&path).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn gadgets_keep_their_places_and_settings() {
    let app = TestApp::new().await;
    let (_, created) = app
        .post(&format!("{API}/dashboard"), json!({ "name": "Board" }))
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    let path = format!("{API}/dashboard/{id}");

    // New gadgets go to the top of their column, unless a row is given.
    let mut ids = Vec::new();
    for (key, column, row) in [
        (FILTER_RESULTS, 0, None),
        (PIE_CHART, 0, None),
        (PIE_CHART, 1, None),
        (FILTER_RESULTS, 0, Some(5)),
    ] {
        let mut body =
            json!({ "module_key": key, "color": "green", "position": { "column": column } });
        if let Some(row) = row {
            body["position"]["row"] = json!(row);
        }
        let (status, added) = app.post(&format!("{path}/gadget"), body).await;
        assert_eq!(status, StatusCode::OK, "{added}");
        ids.push(added["id"].as_i64().unwrap());
    }
    let (_, list) = app.get(&format!("{path}/gadget")).await;
    assert_eq!(
        places(&list),
        [
            (ids[1], 0, 0),
            (ids[0], 0, 1),
            (ids[3], 0, 2),
            (ids[2], 1, 0)
        ]
    );
    assert_eq!(list["gadgets"][0]["title"], "Pie chart");

    // Moving a gadget closes the gap it leaves.
    let (status, _) = app
        .put(
            &format!("{path}/gadget/{}", ids[1]),
            json!({ "title": "By status", "position": { "column": 1, "row": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = app.get(&format!("{path}/gadget")).await;
    assert_eq!(
        places(&list),
        [
            (ids[0], 0, 0),
            (ids[3], 0, 1),
            (ids[2], 1, 0),
            (ids[1], 1, 1)
        ]
    );
    let (_, only) = app.get(&format!("{path}/gadget?gadgetId={}", ids[1])).await;
    assert_eq!(only["gadgets"][0]["title"], "By status");

    // Bad input is refused.
    for body in [
        json!({ "moduleKey": PIE_CHART, "color": "pink" }),
        json!({ "moduleKey": PIE_CHART, "position": { "column": 2 } }),
        json!({ "moduleKey": "com.example:unknown" }),
        json!({}),
    ] {
        let (status, _) = app.post(&format!("{path}/gadget"), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = app
        .post(
            &format!("{path}/gadget"),
            json!({ "uri": "rest/gadgets/1.0/g/custom.xml", "ignoreUriAndModuleKeyValidation": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Settings live in item properties.
    let property = format!("{path}/items/{}/properties/config", ids[1]);
    let (status, _) = app
        .put(
            &property,
            json!({ "statType": "status", "filterId": "10000" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app.put(&property, json!({ "statType": "assignees" })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, read) = app.get(&property).await;
    assert_eq!(read["value"]["statType"], "assignees");
    let (_, keys) = app
        .get(&format!("{path}/items/{}/properties", ids[1]))
        .await;
    assert_eq!(keys["keys"][0]["key"], "config");

    // A copy has the same gadgets and settings, and belongs to whoever copied it.
    let (status, copy) = app
        .post(&format!("{path}/copy"), json!({ "name": "Board copy" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    let copy_id = copy["id"].as_str().unwrap();
    let (_, copied) = app
        .get(&format!(
            "{API}/dashboard/{copy_id}/gadget?moduleKey={PIE_CHART}"
        ))
        .await;
    let copied_pie = copied["gadgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["title"] == "By status")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    assert_ne!(copied_pie, ids[1]);
    let (_, read) = app
        .get(&format!(
            "{API}/dashboard/{copy_id}/items/{copied_pie}/properties/config"
        ))
        .await;
    assert_eq!(read["value"]["statType"], "assignees");

    // One column: gadgets from the second column go to the end of the first.
    let (status, _) = app
        .put(
            &format!("/rest/tessera/1.0/dashboard/{id}/layout"),
            json!({ "layout": "A" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = app.get(&format!("{path}/gadget")).await;
    let columns: Vec<i64> = places(&list).iter().map(|p| p.1).collect();
    assert!(columns.iter().all(|c| *c == 0), "{list}");
    let rows: Vec<i64> = places(&list).iter().map(|p| p.2).collect();
    assert_eq!(rows, (0..rows.len() as i64).collect::<Vec<_>>());

    // Deleting a gadget closes its gap too.
    let (status, _) = app.delete(&format!("{path}/gadget/{}", ids[0])).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = app.get(&format!("{path}/gadget")).await;
    let rows: Vec<i64> = places(&list).iter().map(|p| p.2).collect();
    assert_eq!(rows, (0..rows.len() as i64).collect::<Vec<_>>());
}
