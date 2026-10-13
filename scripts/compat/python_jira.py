# /// script
# requires-python = ">=3.10"
# dependencies = ["jira>=3.10"]
# ///
"""Drives a Tessera server with the official Python `jira` client, unchanged.

Usage: uv run scripts/compat/python_jira.py <base-url> <email> <api-token>

The script expects an empty server with one administrator. It exits non-zero on the first
failed check.
"""

import sys

from jira import JIRA

url, email, token = sys.argv[1:4]
jira = JIRA(server=url, basic_auth=(email, token))


def check(label, condition):
    print(("ok   " if condition else "FAIL ") + label)
    if not condition:
        sys.exit(1)


me = jira.myself()
check("myself", me["emailAddress"] == email)

jira.create_project("COMPAT", name="Compat Project")
check("project created", jira.project("COMPAT").name == "Compat Project")
check("projects listed", [p.key for p in jira.projects()] == ["COMPAT"])
check("issue types", {t.name for t in jira.issue_types()} >= {"Bug", "Task", "Story"})
check("priorities", [p.name for p in jira.priorities()][0] == "Highest")

issue = jira.create_issue(
    project="COMPAT",
    summary="Created by the jira client",
    description="Plain text from v2",
    issuetype={"name": "Bug"},
    labels=["compat"],
)
check("issue created", issue.key == "COMPAT-1")
check("issue fields", issue.fields.issuetype.name == "Bug" and issue.fields.labels == ["compat"])
check("description", jira.issue("COMPAT-1").fields.description == "Plain text from v2")

issue.update(summary="Updated summary", priority={"name": "High"})
issue = jira.issue("COMPAT-1")
check("issue updated", issue.fields.summary == "Updated summary" and issue.fields.priority.name == "High")

comment = jira.add_comment(issue, "A comment")
check("comment added", jira.comment("COMPAT-1", comment.id).body == "A comment")
check("comments listed", len(jira.comments(issue)) == 1)

names = [t["name"] for t in jira.transitions(issue)]
check("transitions", names == ["In Progress", "Done"])
jira.transition_issue(issue, "Done")
issue = jira.issue("COMPAT-1")
check("transitioned", issue.fields.status.name == "Done" and issue.fields.resolution.name == "Done")

jira.assign_issue(issue, me["accountId"])
check("assigned", jira.issue("COMPAT-1").fields.assignee.accountId == me["accountId"])

for i in range(2, 6):
    jira.create_issue(project="COMPAT", summary=f"Task {i}", issuetype={"name": "Task"})
found = jira.search_issues("project = COMPAT AND type = Task ORDER BY key DESC", maxResults=2)
check("search with paging", [i.key for i in found] == ["COMPAT-5", "COMPAT-4"])
found = jira.search_issues("project = COMPAT AND status = Done")
check("search by status", [i.key for i in found] == ["COMPAT-1"])
found = jira.search_issues('summary ~ "Updated" AND assignee = currentUser()')
check("text search and currentUser()", [i.key for i in found] == ["COMPAT-1"])
everything = jira.search_issues("project = COMPAT", maxResults=False)
check("search all pages", len(everything) == 5)

meta = jira.createmeta(projectKeys="COMPAT", issuetypeNames="Task", expand="projects.issuetypes.fields")
check("createmeta", "summary" in meta["projects"][0]["issuetypes"][0]["fields"])

field_names = {f["name"] for f in jira.fields()}
check("fields", {"Summary", "Status", "Assignee"} <= field_names)

jira.issue("COMPAT-5").delete()
check("issue deleted", len(jira.search_issues("project = COMPAT", maxResults=False)) == 4)

print("All checks passed.")
