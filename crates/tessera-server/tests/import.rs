//! Importing Jira and Confluence backups.

mod common;

use std::io::Write;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::json;

/// Writes a backup zip with `entities.xml` and other files.
fn backup(files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tessera-backup-{}.zip",
        uuid::Uuid::new_v4().simple()
    ));
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    path
}

const JIRA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<entity-engine-xml>
  <User id="1" directoryId="1" userName="alice" lowerUserName="alice" active="1" displayName="Alice Moreau" emailAddress="alice@example.com"/>
  <User id="2" directoryId="1" userName="bob" lowerUserName="bob" active="1" displayName="Bob Diaz" emailAddress="bob@example.com"/>
  <ApplicationUser id="10000" userKey="5b10ac8d82e05b22cc7d4ef5" lowerUserName="alice"/>
  <ApplicationUser id="10001" userKey="JIRAUSER10001" lowerUserName="bob"/>
  <Group id="1" groupName="developers" lowerGroupName="developers"/>
  <Membership id="1" parentId="1" childId="2" membershipType="GROUP_USER" parentName="developers" lowerParentName="developers" childName="bob" lowerChildName="bob"/>
  <IssueType id="10001" name="Task"/>
  <IssueType id="10002" name="Sub-task" style="jira_subtask"/>
  <Status id="1" name="To Do" statuscategory="2"/>
  <Status id="10100" name="In Review" statuscategory="4"/>
  <Status id="10001" name="Done" statuscategory="3"/>
  <Priority id="2" name="High"/>
  <Resolution id="10000" name="Done"/>
  <Project id="10000" name="Demo" key="DEMO" lead="5b10ac8d82e05b22cc7d4ef5" description="The demo project" assigneetype="3" projecttype="software" pcounter="5"/>
  <Component id="10000" project="10000" name="Backend"/>
  <Version id="10000" project="10000" name="1.0" released="true" sequence="1"/>
  <Issue id="10001" project="10000" number="1" summary="Fix the login" type="10001" status="10100" priority="2" reporter="5b10ac8d82e05b22cc7d4ef5" assignee="JIRAUSER10001" creator="5b10ac8d82e05b22cc7d4ef5" created="2024-01-02 09:00:00.0" updated="2024-01-05 10:00:00.0">
    <description><![CDATA[h2. Steps
* Open the page
* Click *Login*
{code:java}
login();
{code}]]></description>
  </Issue>
  <Issue id="10002" project="10000" number="2" summary="Write a test" type="10002" status="10001" resolution="10000" resolutiondate="2024-01-06 00:00:00.0" reporter="JIRAUSER10001" created="2024-01-03 09:00:00.0"/>
  <Issue id="10003" project="10000" number="5" summary="Ship it" type="10001" status="1" reporter="5b10ac8d82e05b22cc7d4ef5" created="2024-01-04 09:00:00.0"/>
  <IssueLinkType id="10000" linkname="jira_subtask_link" inward="jira_subtask_inward" outward="jira_subtask_outward" style="jira_subtask"/>
  <IssueLinkType id="10001" linkname="Blocks" inward="is blocked by" outward="blocks"/>
  <IssueLink id="1" linktype="10000" source="10001" destination="10002" sequence="0"/>
  <IssueLink id="2" linktype="10001" source="10001" destination="10003"/>
  <Action id="10000" issue="10001" author="JIRAUSER10001" type="comment" created="2024-01-04 12:00:00.0" body="Looks _fine_ to me"/>
  <Label id="1" issue="10001" label="login"/>
  <NodeAssociation sourceNodeId="10001" sourceNodeEntity="Issue" sinkNodeId="10000" sinkNodeEntity="Component" associationType="IssueComponent"/>
  <NodeAssociation sourceNodeId="10001" sourceNodeEntity="Issue" sinkNodeId="10000" sinkNodeEntity="Version" associationType="IssueFixVersion"/>
  <UserAssociation sourceName="JIRAUSER10001" sinkNodeId="10001" sinkNodeEntity="Issue" associationType="WatchIssue"/>
  <Worklog id="1" issue="10001" author="JIRAUSER10001" body="Investigated" created="2024-01-04 13:00:00.0" startdate="2024-01-04 11:00:00.0" timeworked="3600"/>
  <ChangeGroup id="1" issue="10001" author="5b10ac8d82e05b22cc7d4ef5" created="2024-01-05 10:00:00.0"/>
  <ChangeItem id="1" group="1" fieldtype="jira" field="status" oldvalue="1" oldstring="To Do" newvalue="10100" newstring="In Review"/>
  <CustomField id="10010" customfieldtypekey="com.atlassian.jira.plugin.system.customfieldtypes:float" name="Story Points"/>
  <CustomField id="10011" customfieldtypekey="com.atlassian.jira.plugin.system.customfieldtypes:select" name="Team"/>
  <CustomFieldOption id="10100" customfield="10011" value="Platform" sequence="0"/>
  <CustomFieldValue id="1" issue="10001" customfield="10010" numbervalue="3.0"/>
  <CustomFieldValue id="2" issue="10001" customfield="10011" stringvalue="10100"/>
  <FileAttachment id="10200" issue="10001" mimetype="text/plain" filename="notes.txt" filesize="5" author="JIRAUSER10001" created="2024-01-04 14:00:00.0"/>
</entity-engine-xml>"#;

#[tokio::test]
async fn imports_a_jira_backup() {
    let app = TestApp::new().await;
    let path = backup(&[
        ("entities.xml", JIRA.as_bytes()),
        ("data/attachments/DEMO/10000/DEMO-1/10200", b"hello"),
    ]);
    let summary = tessera_server::import::jira::import(&app.state.db, &app.state.data_dir, &path)
        .await
        .expect("the import");
    let text = summary.to_string();
    assert!(text.contains("issues: 3"), "{text}");
    // Alice and Bob, each known by a user name and a user key.
    assert!(text.contains("users: 2"), "{text}");

    // Keys, fields, and text are kept; wiki markup became ADF.
    let (status, issue) = app.get("/rest/api/3/issue/DEMO-1").await;
    assert_eq!(status, StatusCode::OK, "{issue}");
    let fields = &issue["fields"];
    assert_eq!(fields["summary"], "Fix the login");
    assert_eq!(fields["status"]["name"], "In Review");
    assert_eq!(fields["priority"]["name"], "High");
    assert_eq!(fields["reporter"]["displayName"], "Alice Moreau");
    assert_eq!(fields["reporter"]["accountId"], "5b10ac8d82e05b22cc7d4ef5");
    assert_eq!(fields["assignee"]["displayName"], "Bob Diaz");
    assert_eq!(fields["description"]["content"][0]["type"], "heading");
    assert_eq!(fields["description"]["content"][1]["type"], "bulletList");
    assert_eq!(fields["description"]["content"][2]["type"], "codeBlock");
    assert_eq!(fields["labels"], json!(["login"]));
    assert_eq!(fields["components"][0]["name"], "Backend");
    assert_eq!(fields["fixVersions"][0]["name"], "1.0");
    assert_eq!(fields["created"], "2024-01-02T09:00:00.000+0000");
    assert_eq!(fields["subtasks"][0]["key"], "DEMO-2");
    assert_eq!(fields["issuelinks"][0]["type"]["name"], "Blocks");
    assert_eq!(
        fields["comment"]["comments"][0]["body"]["content"][0]["content"][1]["marks"][0]["type"],
        "em"
    );
    assert_eq!(fields["attachment"][0]["filename"], "notes.txt");
    assert_eq!(fields["worklog"]["worklogs"][0]["timeSpentSeconds"], 3600);

    // Custom fields, history, watchers, and groups.
    let (_, names) = app.get("/rest/api/3/field").await;
    let id_of = |name: &str| {
        names
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(fields[id_of("Story point estimate")], 3.0);
    assert_eq!(fields[id_of("Team")]["value"], "Platform");
    let (_, history) = app.get("/rest/api/3/issue/DEMO-1/changelog").await;
    assert_eq!(history["values"][0]["items"][0]["toString"], "In Review");
    let (_, watchers) = app.get("/rest/api/3/issue/DEMO-1/watchers").await;
    assert_eq!(watchers["watchers"][0]["displayName"], "Bob Diaz");
    let (_, members) = app
        .get("/rest/api/3/group/member?groupname=developers")
        .await;
    assert_eq!(members["values"][0]["displayName"], "Bob Diaz");

    // JQL works on imported data, including the history.
    assert_eq!(
        app.jql("project = DEMO AND status = \"In Review\"").await,
        ["DEMO-1"]
    );
    assert_eq!(
        app.jql("status WAS \"To Do\" AND key = DEMO-1").await,
        ["DEMO-1"]
    );
    assert_eq!(
        app.jql("project = DEMO AND resolution = Done").await,
        ["DEMO-2"]
    );

    // The attachment file came across.
    let attachment = fields["attachment"][0]["content"]
        .as_str()
        .unwrap()
        .replace(common::BASE, "");
    let (status, _, bytes) = app.raw(Method::GET, &attachment, &[], Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"hello");

    // New issues continue after the highest imported number.
    let key = app.issue("DEMO", "After the import", json!({})).await;
    assert_eq!(key, "DEMO-6");

    // Importing again skips the project, which now exists.
    let again = tessera_server::import::jira::import(&app.state.db, &app.state.data_dir, &path)
        .await
        .unwrap();
    assert!(again.to_string().contains("Project DEMO was skipped"));
}

const CONFLUENCE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hibernate-generic datetime="2024-02-01 00:00:00">
<object class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[8a7f808a1]]></id><property name="name"><![CDATA[carol]]></property><property name="email"><![CDATA[carol@example.com]]></property></object>
<object class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id><property name="name"><![CDATA[Handbook]]></property><property name="key"><![CDATA[HB]]></property><property name="description" class="SpaceDescription" package="com.atlassian.confluence.spaces"><id name="id">98306</id></property><property name="homePage" class="Page" package="com.atlassian.confluence.pages"><id name="id">65537</id></property><property name="creator" class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[8a7f808a1]]></id></property><property name="creationDate">2024-01-10 10:00:00.000</property><property name="spaceType"><![CDATA[global]]></property></object>
<object class="SpaceDescription" package="com.atlassian.confluence.spaces"><id name="id">98306</id></object>
<object class="BodyContent" package="com.atlassian.confluence.core"><id name="id">1</id><property name="body"><![CDATA[<p>How we work</p>]]></property><property name="content" class="SpaceDescription" package="com.atlassian.confluence.spaces"><id name="id">98306</id></property><property name="bodyType">2</property></object>
<object class="Page" package="com.atlassian.confluence.pages"><id name="id">65537</id><property name="title"><![CDATA[Handbook Home]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property><property name="version">2</property><property name="creator" class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[8a7f808a1]]></id></property><property name="creationDate">2024-01-10 10:00:00.000</property><property name="lastModificationDate">2024-01-12 10:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="Page" package="com.atlassian.confluence.pages"><id name="id">65540</id><property name="title"><![CDATA[Handbook Home]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property><property name="version">1</property><property name="originalVersion" class="Page" package="com.atlassian.confluence.pages"><id name="id">65537</id></property><property name="creationDate">2024-01-10 10:00:00.000</property><property name="lastModificationDate">2024-01-10 10:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="BodyContent" package="com.atlassian.confluence.core"><id name="id">2</id><property name="body"><![CDATA[<p>Welcome, second version</p>]]></property><property name="content" class="Page" package="com.atlassian.confluence.pages"><id name="id">65537</id></property><property name="bodyType">2</property></object>
<object class="BodyContent" package="com.atlassian.confluence.core"><id name="id">3</id><property name="body"><![CDATA[<p>Welcome</p>]]></property><property name="content" class="Page" package="com.atlassian.confluence.pages"><id name="id">65540</id></property><property name="bodyType">2</property></object>
<object class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id><property name="title"><![CDATA[Onboarding]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property><property name="parent" class="Page" package="com.atlassian.confluence.pages"><id name="id">65537</id></property><property name="version">1</property><property name="creationDate">2024-01-11 10:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="BodyContent" package="com.atlassian.confluence.core"><id name="id">4</id><property name="body"><![CDATA[<p>Day one</p>]]></property><property name="content" class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id></property><property name="bodyType">2</property></object>
<object class="BlogPost" package="com.atlassian.confluence.pages"><id name="id">65539</id><property name="title"><![CDATA[We moved]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property><property name="version">1</property><property name="creationDate">2024-01-15 10:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="Comment" package="com.atlassian.confluence.pages"><id name="id">65541</id><property name="containerContent" class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id></property><property name="version">1</property><property name="creator" class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[8a7f808a1]]></id></property><property name="creationDate">2024-01-11 11:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="BodyContent" package="com.atlassian.confluence.core"><id name="id">5</id><property name="body"><![CDATA[<p>Great page</p>]]></property><property name="content" class="Comment" package="com.atlassian.confluence.pages"><id name="id">65541</id></property><property name="bodyType">2</property></object>
<object class="Comment" package="com.atlassian.confluence.pages"><id name="id">65542</id><property name="containerContent" class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id></property><property name="parent" class="Comment" package="com.atlassian.confluence.pages"><id name="id">65541</id></property><property name="version">1</property><property name="creationDate">2024-01-11 12:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="Label" package="com.atlassian.confluence.labels"><id name="id">1</id><property name="name"><![CDATA[onboarding]]></property><property name="namespace"><![CDATA[global]]></property></object>
<object class="Labelling" package="com.atlassian.confluence.labels"><id name="id">1</id><property name="label" class="Label" package="com.atlassian.confluence.labels"><id name="id">1</id></property><property name="content" class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id></property></object>
<object class="Attachment" package="com.atlassian.confluence.pages"><id name="id">65543</id><property name="title"><![CDATA[plan.txt]]></property><property name="containerContent" class="Page" package="com.atlassian.confluence.pages"><id name="id">65538</id></property><property name="version">1</property><property name="creationDate">2024-01-11 13:00:00.000</property><property name="contentStatus"><![CDATA[current]]></property></object>
<object class="ContentProperty" package="com.atlassian.confluence.content"><id name="id">1</id><property name="name"><![CDATA[FILESIZE]]></property><property name="longValue">4</property><property name="content" class="Attachment" package="com.atlassian.confluence.pages"><id name="id">65543</id></property></object>
<object class="ContentProperty" package="com.atlassian.confluence.content"><id name="id">2</id><property name="name"><![CDATA[MEDIA_TYPE]]></property><property name="stringValue"><![CDATA[text/plain]]></property><property name="content" class="Attachment" package="com.atlassian.confluence.pages"><id name="id">65543</id></property></object>
<object class="SpacePermission" package="com.atlassian.confluence.security"><id name="id">1</id><property name="type"><![CDATA[VIEWSPACE]]></property><property name="group"><![CDATA[confluence-users]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property></object>
<object class="SpacePermission" package="com.atlassian.confluence.security"><id name="id">2</id><property name="type"><![CDATA[EDITSPACE]]></property><property name="group"><![CDATA[confluence-users]]></property><property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property></object>
</hibernate-generic>"#;

#[tokio::test]
async fn imports_a_confluence_backup() {
    let app = TestApp::new().await;
    let path = backup(&[
        ("entities.xml", CONFLUENCE.as_bytes()),
        ("attachments/65538/65543/1", b"plan"),
    ]);
    let summary =
        tessera_server::import::confluence::import(&app.state.db, &app.state.data_dir, &path)
            .await
            .expect("the import");
    let text = summary.to_string();
    assert!(text.contains("pages and blog posts: 3"), "{text}");
    const API: &str = "/wiki/rest/api";

    let (status, space) = app
        .get(&format!("{API}/space/HB?expand=description.plain,homepage"))
        .await;
    assert_eq!(status, StatusCode::OK, "{space}");
    assert_eq!(space["name"], "Handbook");
    assert_eq!(space["description"]["plain"]["value"], "How we work");
    assert_eq!(space["homepage"]["title"], "Handbook Home");

    // Both versions of the home page are kept.
    let home = space["homepage"]["id"].as_str().unwrap().to_string();
    let (_, versions) = app.get(&format!("{API}/content/{home}/version")).await;
    assert_eq!(versions["size"], 2, "{versions}");
    let (_, old) = app
        .get(&format!(
            "{API}/content/{home}?version=1&expand=body.storage"
        ))
        .await;
    assert_eq!(old["body"]["storage"]["value"], "<p>Welcome</p>");

    // The page tree, comments with a reply, labels, and attachments.
    let (_, children) = app.get(&format!("{API}/content/{home}/child/page")).await;
    assert_eq!(children["results"][0]["title"], "Onboarding");
    let page = children["results"][0]["id"].as_str().unwrap().to_string();
    let (_, comments) = app
        .get(&format!(
            "{API}/content/{page}/child/comment?depth=all&expand=body.storage,history"
        ))
        .await;
    assert_eq!(comments["size"], 2);
    assert_eq!(
        comments["results"][0]["history"]["createdBy"]["email"],
        "carol@example.com"
    );
    let (_, labels) = app.get(&format!("{API}/content/{page}/label")).await;
    assert_eq!(labels["results"][0]["name"], "onboarding");
    let (_, files) = app
        .get(&format!("{API}/content/{page}/child/attachment"))
        .await;
    assert_eq!(files["results"][0]["title"], "plan.txt");
    let link = files["results"][0]["_links"]["download"].as_str().unwrap();
    let (status, _, bytes) = app
        .raw(Method::GET, &format!("/wiki{link}"), &[], Vec::new())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"plan");

    // The blog post, CQL, and the space's permissions.
    let (_, blog) = app.get(&format!("{API}/space/HB/content/blogpost")).await;
    assert_eq!(blog["results"][0]["title"], "We moved");
    let (_, found) = app
        .get(&format!(
            "{API}/content/search?cql=space%3DHB%20and%20text~%22Day%20one%22"
        ))
        .await;
    assert_eq!(found["results"][0]["title"], "Onboarding");
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let (status, _) = app
        .request(
            Method::GET,
            &format!("{API}/content/{page}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}
