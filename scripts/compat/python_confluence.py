# /// script
# requires-python = ">=3.10"
# dependencies = ["atlassian-python-api>=3.41,<4"]
# ///
"""Drives a Tessera server with the classic Confluence client from `atlassian-python-api` 3.x,
unchanged. It uses the v1 REST API, like most existing scripts.

Usage: uv run scripts/compat/python_confluence.py <base-url> <email> <api-token>

The script expects a server with no spaces. It exits non-zero on the first failed check.
"""

import os
import sys
import tempfile

from atlassian import Confluence

url, email, token = sys.argv[1:4]
wiki = Confluence(url=url + "/wiki", username=email, password=token, cloud=True)


def check(label, condition):
    print(("ok   " if condition else "FAIL ") + label)
    if not condition:
        sys.exit(1)


wiki.create_space("COMPAT", "Compat Space")
space = wiki.get_space("COMPAT", expand="description.plain,homepage")
check("space created", space["name"] == "Compat Space")
check("space home page", space["homepage"]["title"] == "Compat Space Home")
spaces = wiki.get_all_spaces(start=0, limit=50)
check("spaces listed", [s["key"] for s in spaces["results"]] == ["COMPAT"])

page = wiki.create_page("COMPAT", "Getting started", "<p>Hello from the client</p>")
check("page created", page["title"] == "Getting started" and page["version"]["number"] == 1)
page_id = page["id"]
check("page exists", wiki.page_exists("COMPAT", "Getting started"))
check("page id by title", wiki.get_page_id("COMPAT", "Getting started") == page_id)

found = wiki.get_page_by_title("COMPAT", "Getting started", expand="body.storage")
check("page by title", found["body"]["storage"]["value"] == "<p>Hello from the client</p>")

wiki.update_page(page_id, "Getting started", "<p>Second version</p>", version_comment="Edit")
updated = wiki.get_page_by_id(page_id, expand="body.storage,version")
check("page updated", updated["version"]["number"] == 2)
check("new body", updated["body"]["storage"]["value"] == "<p>Second version</p>")
check("history", wiki.get_content_history(page_id)["createdBy"]["accountId"] != "")

child = wiki.create_page("COMPAT", "Install", "<p>Steps</p>", parent_id=page_id)
children = wiki.get_page_child_by_type(page_id, type="page")
check("child pages", [c["title"] for c in children] == ["Install"])
ancestors = wiki.get_page_ancestors(child["id"])
check("ancestors", [a["title"] for a in ancestors] == ["Getting started"])

wiki.set_page_label(page_id, "howto")
labels = wiki.get_page_labels(page_id)
check("labels", [l["name"] for l in labels["results"]] == ["howto"])

wiki.add_comment(page_id, "<p>Looks good</p>")
comments = wiki.get_page_comments(page_id, expand="body.storage", depth="")
check("comments", comments["results"][0]["body"]["storage"]["value"] == "<p>Looks good</p>")

notes = os.path.join(tempfile.mkdtemp(), "notes.txt")
with open(notes, "w") as f:
    f.write("first")
wiki.attach_file(notes, name="notes.txt", content_type="text/plain", page_id=page_id, comment="Notes")
with open(notes, "w") as f:
    f.write("second")
# The client finds the existing file and uploads a new version of it.
wiki.attach_file(notes, name="notes.txt", content_type="text/plain", page_id=page_id)
files = wiki.get_attachments_from_content(page_id, expand="version")["results"]
check("attachment uploaded", [a["title"] for a in files] == ["notes.txt"])
check("attachment versioned", files[0]["version"]["number"] == 2)
link = files[0]["_links"]["download"]
check("attachment download", wiki.get(link.lstrip("/"), not_json_response=True) == b"second")

results = wiki.cql('space = COMPAT and type = page and title ~ "install"')
check("cql search", [r["content"]["title"] for r in results["results"]] == ["Install"])
pages = wiki.get_all_pages_from_space("COMPAT", start=0, limit=50)
check("pages in space", {p["title"] for p in pages} == {"Compat Space Home", "Getting started", "Install"})

me = wiki.get("rest/api/user/current")["accountId"]
wiki.put(
    "rest/api/content/{}/restriction".format(page_id),
    data=[{"operation": "update", "restrictions": {"user": [{"type": "known", "accountId": me}]}}],
)
restrictions = wiki.get_all_restrictions_for_content(page_id)
check("restrictions", [u["accountId"] for u in restrictions["update"]["restrictions"]["user"]["results"]] == [me])
check("no view restrictions", restrictions["read"]["restrictions"]["user"]["size"] == 0)

blueprints = wiki.get_blueprint_templates()
check("blueprints", "Meeting notes" in [t["name"] for t in blueprints])
made = wiki.create_or_update_template(
    "Compat template",
    {"storage": {"value": "<p>From a template</p>", "representation": "storage"}},
    description="Made by the client",
    space="COMPAT",
)
check("template created", wiki.get_content_template(made["templateId"])["body"]["storage"]["value"] == "<p>From a template</p>")
check("space templates", [t["name"] for t in wiki.get_content_templates("COMPAT")] == ["Compat template"])
wiki.create_or_update_template(
    "Compat template v2",
    {"storage": {"value": "<p>Changed</p>", "representation": "storage"}},
    template_id=made["templateId"],
    space="COMPAT",
)
check("template updated", wiki.get_content_template(made["templateId"])["name"] == "Compat template v2")
wiki.remove_template(made["templateId"])
check("template removed", wiki.get_content_templates("COMPAT") == [])

wiki.remove_page(child["id"])
check("page trashed", not wiki.page_exists("COMPAT", "Install"))

print("All checks passed.")
