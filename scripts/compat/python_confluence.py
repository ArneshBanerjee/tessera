# /// script
# requires-python = ">=3.10"
# dependencies = ["atlassian-python-api>=3.41,<4"]
# ///
"""Drives a Tessera server with the classic Confluence client from `atlassian-python-api` 3.x,
unchanged. It uses the v1 REST API, like most existing scripts.

Usage: uv run scripts/compat/python_confluence.py <base-url> <email> <api-token>

The script expects a server with no spaces. It exits non-zero on the first failed check.
"""

import sys

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

results = wiki.cql('space = COMPAT and type = page and title ~ "install"')
check("cql search", [r["content"]["title"] for r in results["results"]] == ["Install"])
pages = wiki.get_all_pages_from_space("COMPAT", start=0, limit=50)
check("pages in space", {p["title"] for p in pages} == {"Compat Space Home", "Getting started", "Install"})

wiki.remove_page(child["id"])
check("page trashed", not wiki.page_exists("COMPAT", "Install"))

print("All checks passed.")
