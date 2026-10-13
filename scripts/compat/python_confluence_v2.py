# /// script
# requires-python = ">=3.10"
# dependencies = ["atlassian-python-api>=5,<6"]
# ///
"""Drives a Tessera server with the Confluence Cloud v2 client from `atlassian-python-api` 5.x,
unchanged.

Usage: uv run scripts/compat/python_confluence_v2.py <base-url> <email> <api-token>

The script expects a server without a space named V2COMPAT. It exits non-zero on the first
failed check.
"""

import sys

from atlassian import Confluence

url, email, token = sys.argv[1:4]
wiki = Confluence(url=url + "/wiki", username=email, password=token, api_version=2)


def check(label, condition):
    print(("ok   " if condition else "FAIL ") + label)
    if not condition:
        sys.exit(1)


created = wiki.create_space("V2COMPAT", "V2 Compat")
check("v2 space created", created["key"] == "V2COMPAT")
space = wiki.get_space_by_key("V2COMPAT")
space_id = space["id"]
check("v2 space by key", space["name"] == "V2 Compat")
check("v2 spaces listed", "V2COMPAT" in [s["key"] for s in wiki.get_spaces(keys=["V2COMPAT"])])

page = wiki.create_page(space_id=space_id, title="Overview", body="<p>First</p>")
page_id = page["id"]
check("v2 page created", page["title"] == "Overview" and page["version"]["number"] == 1)

read = wiki.get_page_by_id(page_id, body_format="storage")
check("v2 page body", read["body"]["storage"]["value"] == "<p>First</p>")

wiki.update_page(page_id, title="Overview", body="<p>Second</p>")
read = wiki.get_page_by_id(page_id, body_format="storage")
check("v2 page updated", read["version"]["number"] == 2 and read["body"]["storage"]["value"] == "<p>Second</p>")

child = wiki.create_page(space_id=space_id, title="Details", body="<p>More</p>", parent_id=page_id)
check("v2 child pages", [c["title"] for c in wiki.get_child_pages(page_id)] == ["Details"])

titles = [p["title"] for p in wiki.get_pages(space_id=space_id, sort="title")]
check("v2 pages in space", titles == ["Details", "Overview", "V2 Compat Home"])

# The client's create_page_footer_comment posts to /comments, which Confluence does not have,
# so this uses the client's request method with the real endpoint.
comment = wiki.post(
    "api/v2/footer-comments",
    data={"pageId": page_id, "body": {"representation": "storage", "value": "<p>Nice page</p>"}},
)
check("v2 footer comment", comment["pageId"] == page_id)
comments = wiki.get_page_footer_comments(page_id, body_format="storage")
check("v2 footer comments", comments[0]["body"]["storage"]["value"] == "<p>Nice page</p>")

versions = wiki.get_page_versions(page_id)
check("v2 versions", [v["number"] for v in versions["results"]] == [2, 1])

check("v2 page deleted", wiki.delete_page(child["id"]))

print("All checks passed.")
