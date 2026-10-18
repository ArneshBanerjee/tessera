# Tessera

An open-source, clean-room clone of the Atlassian suite, written in Rust.

Tessera aims for 1:1 parity with Jira and Confluence: the same features, the same workflows, and the same REST APIs. Your existing scripts, clients, and integrations work after you change the server URL. Tessera runs as one binary on Linux, macOS, and Windows, and you host it yourself.

> **Status: pre-alpha.** The Jira API, a first web app, and the Confluence v1 and v2 APIs work. Do not use it for real work yet.

Tessera is not affiliated with or endorsed by Atlassian. Jira, Confluence, and Atlassian are trademarks of Atlassian.

Tessera is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE). If you run a modified Tessera as a service, you must publish your changes.

## What works today

The Jira REST API, under `/rest/api/2`, `/rest/api/3`, and `/rest/api/latest`, and the Jira Software API under `/rest/agile/1.0`:

| Area | Endpoints |
| --- | --- |
| Auth | Basic auth with email and API token (or password). Cookie sessions at `/rest/auth/1/session`. |
| Users | `myself`, `user`, `user/search`, `users/search`, `user/assignable/search` |
| Projects | Create, read, update, delete, `project/search`, `project/{key}/statuses`, project categories |
| Issues | Create, read, edit, delete, assign, `changelog`, `createmeta`, `editmeta`, the `fields` and `expand` parameters |
| Workflow | Transitions, with resolution set and cleared when entering and leaving done statuses. Custom statuses (`/statuses`), workflows with initial, directed, and global transitions (`/workflow`), and workflow schemes that pick a workflow per issue type (`/workflowscheme`). When a project changes workflows, issues move to a status of the same category, and boards get columns for new statuses. |
| Comments | Create, read, update, delete, with author checks |
| Components | Create, read, update, delete (moving issues), issue counts, automatic assignee from component or project lead |
| Versions | Create, read, update, delete (moving issues), reorder, issue counts, release and archive flags |
| Watchers and votes | Watch, unwatch, vote, unvote, with Jira's rules (no votes on your own or resolved issues). Creators and commenters watch automatically. |
| Issue links | Link types, create and delete links (with Jira's inward and outward naming), `issuelinks` on issues |
| Worklogs | Log, edit, and delete work, with remaining estimate adjustment (`auto`, `new`, `manual`, `leave`) and time tracking fields |
| Attachments | Upload (multipart, with Jira's XSRF header check), download, delete, metadata |
| Saved filters | Create, read, update, delete, search, favourites, sharing (global, signed-in users, project), and `filter = ...` in JQL |
| Dashboards | Create, read, update, delete, copy, search, sharing as for filters, gadgets (add, move, recolor, remove), gadget item properties, and the system dashboard. Favourites and column layouts at `/rest/tessera/1.0/dashboard/{id}`. |
| Fields | System fields, custom fields (text, number, date, user, select, multi-select, labels) and their options |
| Search | `search` and `search/jql`, with paging, `names`, and the rules below |
| Permissions | Permission schemes with grants to everyone, groups, users, project roles, the project lead, reporters, and assignees; project roles and their members; groups; `mypermissions`. Projects a user cannot browse are hidden from every endpoint and from search, as in Jira. Issue security schemes with levels and members (`/issuesecurityschemes`, `/securitylevel`): an issue at a level is hidden from everyone outside it, in search, links, subtasks, and email. Subtasks share their parent's level, and `level` works in JQL. |
| Metadata | Issue types, statuses, status categories, priorities, resolutions, notification and security schemes |

**Jira Software** (`/rest/agile/1.0`): scrum and kanban boards with columns, board issues and backlog, sprints (create, start, complete, delete, swap, move issues), global ranking, epics (issues, colors, done, rank), estimation, and board and sprint properties. Software projects created from scrum, kanban, or basic templates get a board, as in Jira. Reports rebuild each sprint from issue history: the sprint report (completed, not completed, removed, and added issues) and velocity at `/rest/greenhopper/1.0/rapid/charts`, and a burndown series at `/rest/tessera/1.0/reports/burndown`. Boards have quick filters (`/board/{id}/quickfilter`; new boards get Only My Issues and Recently Updated), managed at `/rest/tessera/1.0/board/{id}/quickfilter`. Sprint, Rank, and Story point estimate are built-in fields.

**JQL** follows Jira's rules: `=`, `!=`, `IN`, `NOT IN`, `IS EMPTY`, `~`, comparisons, `WAS`, `WAS IN`, `WAS NOT`, `CHANGED` with `FROM`, `TO`, `BY`, `AFTER`, `BEFORE`, `ON`, `DURING`, relative dates (`-7d`), date functions (`startOfWeek(-1)`, `endOfMonth()`), `currentUser()`, custom fields by name or `cf[id]`, and `ORDER BY`. It also searches components, fix and affected versions (with `releasedVersions()`, `unreleasedVersions()`, `latestReleasedVersion()`, `earliestUnreleasedVersion()`), watchers and voters (`watchedIssues()`, `votedIssues()`), links (`linkedIssues()`, `issueLinkType`), time tracking (`timespent > 4h`), worklogs, attachments, sprints (`openSprints()`, `closedSprints()`, `futureSprints()`), saved filters, `"Epic Link"`, `standardIssueTypes()`, and `ORDER BY Rank`. Errors use Jira's wording.

Rich text is stored as ADF. The v3 API returns ADF, and v2 returns plain text.

**Notifications**: with an SMTP server set, Tessera emails the assignee, reporter, and watchers when an issue is created, edited, assigned, transitioned, commented on, or deleted, as Jira's default notification scheme does. For Confluence, the watchers of a page and of its space hear about new pages, edits (not minor edits), and comments; you watch pages you create, edit, or comment on (`/user/watch/content` and `/user/watch/space` manage watching). Nobody gets mail about their own changes or about things they cannot see.

**Web app**: log in, your work, projects (create from scrum, kanban, or task tracking templates), the backlog (quick create, sprints, drag issues into sprints, start sprint), the board (drag cards to change status, quick filters, complete sprint), the issue view (edit summary and description in Markdown, transitions, assignee, reporter, priority, labels, due date, story points, subtasks, issue links, attachments, comments, history, watch, delete), JQL search with saved filters and favourites, quick search, reports (burndown, sprint report, and velocity), and dashboards (create, share, copy, star, column layouts, and gadgets: introduction, assigned to me, watched issues, filter results, pie chart, and created vs. resolved chart). For Confluence: spaces, a page tree, a blog for each space, pages and blog posts with their history and old versions (with restore), a Markdown editor (with tables and task lists) that saves pages as ADF, page templates and blueprints (with images from attachments), attachments, labels, threaded comments, page restrictions, and space settings with a permissions grid. Your profile page manages your name, time zone, password, and API tokens. Project settings manage details and who holds each project role, and administrators manage users, groups, statuses, workflows, workflow schemes, and issue security schemes. The server serves the app from its own binary.

**Confluence** (`/wiki/rest/api`, the v1 API): spaces (with home pages), pages, blog posts, and comments with replies. Every version is kept: updates must name the next version number (409 otherwise, as in Confluence), old versions can be read and restored. The page tree: children, descendants, ancestors, and moving pages before, after, or under others. Page templates and blueprints (`/template`). Attachments with versions (upload, new versions, download, images in pages), trash and purge, labels, history, and content body conversion. Bodies use the storage format; the `view` format is sanitized HTML. Errors use Confluence's format. Access follows Confluence: space permissions for users, groups, and anonymous visitors, and page restrictions (`/content/{id}/restriction`) that limit viewing or editing to named users and groups. A view restriction also hides every page below it, in lists, search, and email.

**Confluence v2** (`/wiki/api/v2`): spaces, pages, blog posts, footer comments with replies, attachments, versions, labels, children, descendants, and ancestors, with string ids, `body-format`, sorting, cursor paging, and v2 errors. Bodies can be read and written as storage or as ADF (`atlas_doc_format`); Tessera converts between them, macros included (code, panels, expand, status, tasks, mentions, emoji, images).

**CQL** reuses the JQL parser: `type`, `space` (and `space.id`), `title`, `text`, `id`, `ancestor`, `parent`, `container`, `label`, `creator`, `contributor`, `created`, and `lastmodified`, with `~`, `IN`, comparisons, `currentUser()`, `now("-4w")`, `startOfDay()` and the other date functions, and `ORDER BY`. `/content/search` returns content; `/search` returns search results, including spaces for `type = space`.

### Not yet built

More dashboard gadgets, in-app notifications, Confluence inline comments, a rich-text editor, and import of Atlassian backups.

## Compatibility

`scripts/compat/run.sh` starts a throwaway server and drives it with unchanged clients. The official Python [`jira`](https://pypi.org/project/jira/) client covers projects, issues, comments, transitions, assignment, JQL search with paging, create metadata, fields, components, versions, watchers, links, worklogs, attachments, saved filters, dashboards and gadgets, issue security levels, boards, sprints, ranking, the backlog, and deletion. The classic Confluence client from [`atlassian-python-api`](https://pypi.org/project/atlassian-python-api/) 3.x covers spaces, pages, updates, history, the page tree, attachments, labels, comments, CQL, restrictions, and the trash over v1, and its 5.x Cloud client covers spaces, pages, bodies, children, footer comments, versions, and deletion over v2. CI runs all three on every push.

## Run it

Build the web app first, so the server can include it. You need [Trunk](https://trunkrs.dev) and the `wasm32-unknown-unknown` target.

```bash
rustup target add wasm32-unknown-unknown
(cd crates/tessera-web && trunk build --release)
cargo build --release
./target/release/tessera create-user --email you@example.com --name "Your Name" --password change-me --admin
./target/release/tessera serve
```

The server listens on `127.0.0.1:8080` and stores data in `tessera.db`. Open `http://127.0.0.1:8080` in a browser to use the web app. If you skip the web app build, the server still serves the API.

| Setting | Flag | Variable | Default |
| --- | --- | --- | --- |
| Database | `--database` | `TESSERA_DATABASE` | `sqlite://tessera.db` |
| Listen address | `--addr` | `TESSERA_ADDR` | `127.0.0.1:8080` |
| Public URL | `--base-url` | `TESSERA_BASE_URL` | `http://<addr>` |
| Data directory (attachments) | `--data-dir` | `TESSERA_DATA_DIR` | `tessera-data` |
| SMTP server for notifications | `--smtp-url` | `TESSERA_SMTP_URL` | none (no email) |
| Sender of notifications | `--mail-from` | `TESSERA_MAIL_FROM` | `Tessera <tessera@localhost>` |

SMTP URLs look like `smtp://user:password@mail.example.com:587` (STARTTLS), `smtps://...:465` (TLS), or `smtp+plain://localhost:1025` (no TLS, for a local relay).

On a fresh database, `TESSERA_ADMIN_EMAIL` and `TESSERA_ADMIN_PASSWORD` create the first administrator at startup.

Create an API token for scripts and clients:

```bash
./target/release/tessera create-token --email you@example.com
```

## Test it

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/compat/run.sh
```

To work on the web app, run the server on port 8080 and `trunk serve` in `crates/tessera-web`. Trunk sends API calls to the server.

## Layout

| Crate | Purpose |
| --- | --- |
| `tessera-jql` | JQL lexer and parser |
| `tessera-adf` | Atlassian Document Format types, and conversion to and from plain text, Markdown, and Confluence's storage format |
| `tessera-server` | Storage (SQLite), the JQL to SQL compiler, the REST API, and serving the web app |
| `tessera-web` | The web app (Leptos, compiled to WebAssembly) |

## Clean-room rules

1. Build only from public sources: Atlassian's published API specs, public documentation, and the observed behavior of the products.
2. Never use leaked, decompiled, or otherwise private Atlassian code.
3. Never copy Atlassian logos, icons, or design-system assets. Tessera's icons and avatars are its own.
4. Prove compatibility with tests. Existing Jira and Confluence clients must work against Tessera with no changes.

`scripts/fetch-specs.sh` downloads Atlassian's public OpenAPI specs into `spec/`. The specs are Atlassian's documents, so the repository does not include them.

## Roadmap

| Milestone | Scope |
| --- | --- |
| 1. Foundation | Workspace, JQL parser, server. **Done.** |
| 2. Jira core API | Users, projects, issues, workflows, comments, custom fields, JQL search, history. **Done.** |
| 3. Jira depth | Attachments, issue links, watchers, votes, worklogs, components, versions, groups, project roles, and permission schemes. **Done.** |
| 4. Boards | Saved filters, scrum and kanban boards, sprints, backlog, ranking, epics, and the Agile REST API. **Done.** |
| 5. Web UI | Leptos (Rust compiled to WebAssembly) app. **First version done.** A Tauri desktop app follows. |
| 6. Confluence core | Spaces, page tree, versions, comments, labels, attachments, CQL, the v1 and v2 APIs, and the web app with a Markdown editor. **In progress.** A rich-text editor and inline comments follow. |
| 7. Migration | Import of Jira Cloud and Confluence backups. |
| 8. Launch | Jira and Confluence together. Service Management, Trello-style boards, and Statuspage follow. |
