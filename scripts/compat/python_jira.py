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

component = jira.create_component("Backend", "COMPAT")
check("component created", [c.name for c in jira.project_components("COMPAT")] == ["Backend"])
version = jira.create_version("1.0", "COMPAT", releaseDate="2026-12-01")
check("version created", [v.name for v in jira.project_versions("COMPAT")] == ["1.0"])
issue = jira.issue("COMPAT-2")
issue.update(fields={"components": [{"name": "Backend"}], "fixVersions": [{"name": "1.0"}]})
issue = jira.issue("COMPAT-2")
check("components and versions on an issue",
      issue.fields.components[0].name == "Backend" and issue.fields.fixVersions[0].name == "1.0")
check("search by component", [i.key for i in jira.search_issues("component = Backend")] == ["COMPAT-2"])

check("creator watches", jira.watchers("COMPAT-2").watchCount == 1)
jira.remove_watcher("COMPAT-2", me["accountId"])
check("watcher removed", jira.watchers("COMPAT-2").watchCount == 0)
jira.add_watcher("COMPAT-2", me["accountId"])
check("watcher added", jira.watchers("COMPAT-2").isWatching)

check("link types", {t.name for t in jira.issue_link_types()} >= {"Blocks", "Relates"})
jira.create_issue_link("Blocks", "COMPAT-3", "COMPAT-2")
links = jira.issue("COMPAT-2").fields.issuelinks
check("issue link", len(links) == 1 and links[0].type.name == "Blocks")

jira.add_worklog("COMPAT-2", timeSpent="2h", comment="Worked")
worklogs = jira.worklogs("COMPAT-2")
check("worklog", len(worklogs) == 1 and worklogs[0].timeSpentSeconds == 7200)

import io
attachment = jira.add_attachment("COMPAT-2", attachment=io.BytesIO(b"compat file"), filename="compat.txt")
check("attachment uploaded", attachment.filename == "compat.txt" and attachment.size == 11)
check("attachment content", jira.attachment(attachment.id).get() == b"compat file")

saved = jira.create_filter(name="Compat tasks", jql="project = COMPAT AND type = Task", favourite=True)
check("filter created", jira.filter(saved.id).jql == "project = COMPAT AND type = Task")
check("favourite filters", any(f.id == saved.id for f in jira.favourite_filters()))
board = jira.create_board("Compat board", saved.id, preset="scrum", location_type="project", location_id="COMPAT")
check("board created", any(b.id == board.id for b in jira.boards()))
user_board = jira.create_board("My board", saved.id)
check("user board created", user_board.name == "My board")
sprint = jira.create_sprint("Compat sprint", board.id)
jira.add_issues_to_sprint(sprint.id, ["COMPAT-2", "COMPAT-3"])
check("issues in sprint", len(jira.search_issues(f"sprint = {sprint.id}")) == 2)
jira.update_sprint(sprint.id, name="Compat sprint", state="active",
                   startDate="2026-10-13T09:00:00.000+0000", endDate="2026-10-27T09:00:00.000+0000")
check("sprint started", jira.sprint(sprint.id).state.lower() == "active")
check("board sprints", [s.name for s in jira.sprints(board.id)] == ["Compat sprint"])
jira.rank("COMPAT-4", next_issue="COMPAT-2")
ranked = [i.key for i in jira.search_issues("project = COMPAT ORDER BY Rank ASC")]
check("rank", ranked.index("COMPAT-4") == ranked.index("COMPAT-2") - 1)
jira.move_to_backlog(["COMPAT-3"])
check("moved to backlog", [i.key for i in jira.search_issues("sprint in openSprints()")] == ["COMPAT-2"])

jira.issue("COMPAT-5").delete()
check("issue deleted", len(jira.search_issues("project = COMPAT", maxResults=False)) == 4)


perms = jira.my_permissions(projectKey="COMPAT", permissions="BROWSE_PROJECTS,DELETE_ISSUES")
check("my permissions", perms["permissions"]["BROWSE_PROJECTS"]["havePermission"])
roles = jira.project_roles("COMPAT")
check("project roles", {"Administrators", "Members", "Viewers"} <= set(roles))
admins = jira.project_role("COMPAT", roles["Administrators"]["id"])
check("project role actors", [a.actorUser.accountId for a in admins.actors] == [me["accountId"]])
jira.add_group("compat-devs")
jira.add_user_to_group(me["accountId"], "compat-devs")
check("groups", "compat-devs" in jira.groups())
check("group members", list(jira.group_members("compat-devs")) != [])

check("system dashboard", [d.id for d in jira.dashboards()] == ["10000"])
board = jira.create_dashboard("Compat dashboard", description="From the client")
check("dashboard created", jira.dashboard(board.id).name == "Compat dashboard")
check("my dashboards", [d.name for d in jira.dashboards(filter="my")] == ["Compat dashboard"])
keys = [g.moduleKey for g in jira.all_dashboard_gadgets()]
check("available gadgets", "com.atlassian.jira.gadgets:filter-results-dashboard-item" in keys)
gadget = jira.add_gadget_to_dashboard(
    board.id,
    color="green",
    module_key="com.atlassian.jira.gadgets:filter-results-dashboard-item",
    position={"column": 0, "row": 0},
    title="Compat results",
)
jira.set_dashboard_item_property(board.id, gadget.id, "config", {"filterId": saved.id})
gadgets = jira.dashboard_gadgets(board.id)
check("dashboard gadgets", [g.title for g in gadgets] == ["Compat results"])
check("gadget settings", gadgets[0].item_properties[0].value.filterId == saved.id)
copy = jira.copy_dashboard(board.id, name="Compat copy")
check("dashboard copied", [g.title for g in jira.dashboard(copy.id).gadgets] == ["Compat results"])
jira.update_dashboard_automatic_refresh_minutes(board.id, 15)
check("dashboard refresh", jira.dashboard(board.id).automaticRefreshMs == 15 * 60000)

print("All checks passed.")
