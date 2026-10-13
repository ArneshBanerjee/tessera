# Tessera

An open-source, clean-room clone of the Atlassian suite, written in Rust.

Tessera aims for 1:1 parity with Jira and Confluence: the same features, the same workflows, and the same REST APIs. Your existing scripts, clients, and integrations work after you change the server URL. Tessera runs as one binary on Linux, macOS, and Windows, and you host it yourself.

> **Status: pre-alpha.** The Jira core API works. There is no web UI yet. Do not use it for real work yet.

Tessera is not affiliated with or endorsed by Atlassian. Jira, Confluence, and Atlassian are trademarks of Atlassian.

Tessera is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE). If you run a modified Tessera as a service, you must publish your changes.

## What works today

The Jira REST API, under `/rest/api/2`, `/rest/api/3`, and `/rest/api/latest`:

| Area | Endpoints |
| --- | --- |
| Auth | Basic auth with email and API token (or password). Cookie sessions at `/rest/auth/1/session`. |
| Users | `myself`, `user`, `user/search`, `users/search`, `user/assignable/search` |
| Projects | Create, read, update, delete, `project/search`, `project/{key}/statuses`, project categories |
| Issues | Create, read, edit, delete, assign, `changelog`, `createmeta`, `editmeta`, the `fields` and `expand` parameters |
| Workflow | Transitions, with resolution set and cleared as in Jira's simplified workflow |
| Comments | Create, read, update, delete, with author checks |
| Fields | System fields, custom fields (text, number, date, user, select, multi-select, labels) and their options |
| Search | `search` and `search/jql`, with paging, `names`, and the rules below |
| Metadata | Issue types, statuses, status categories, priorities, resolutions, permission, notification and security schemes |

**JQL** follows Jira's rules: `=`, `!=`, `IN`, `NOT IN`, `IS EMPTY`, `~`, comparisons, `WAS`, `WAS IN`, `WAS NOT`, `CHANGED` with `FROM`, `TO`, `BY`, `AFTER`, `BEFORE`, `ON`, `DURING`, relative dates (`-7d`), date functions (`startOfWeek(-1)`, `endOfMonth()`), `currentUser()`, custom fields by name or `cf[id]`, and `ORDER BY`. Errors use Jira's wording.

Rich text is stored as ADF. The v3 API returns ADF, and v2 returns plain text.

### Not yet built

Web UI, boards and sprints, attachments, issue links, watchers, votes, worklogs, components, versions, project roles and fine-grained permissions, Confluence, and import of Atlassian backups. Issues return these fields empty, so clients do not break.

## Compatibility

`scripts/compat/run.sh` starts a throwaway server and drives it with the official Python [`jira`](https://pypi.org/project/jira/) client, unchanged: projects, issues, comments, transitions, assignment, JQL search with paging, create metadata, fields, and deletion. CI runs it on every push.

## Run it

```bash
cargo build --release
./target/release/tessera create-user --email you@example.com --name "Your Name" --password change-me --admin
./target/release/tessera serve
```

The server listens on `127.0.0.1:8080` and stores data in `tessera.db`.

| Setting | Flag | Variable | Default |
| --- | --- | --- | --- |
| Database | `--database` | `TESSERA_DATABASE` | `sqlite://tessera.db` |
| Listen address | `--addr` | `TESSERA_ADDR` | `127.0.0.1:8080` |
| Public URL | `--base-url` | `TESSERA_BASE_URL` | `http://<addr>` |

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

## Layout

| Crate | Purpose |
| --- | --- |
| `tessera-jql` | JQL lexer and parser |
| `tessera-adf` | Atlassian Document Format types and plain-text conversion |
| `tessera-server` | Storage (SQLite), the JQL to SQL compiler, and the REST API |

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
| 3. Jira depth | Attachments, issue links, watchers, votes, worklogs, components, versions, roles, permissions. |
| 4. Boards | Scrum and Kanban boards, sprints, backlog, and the Agile REST API. |
| 5. Web UI | Leptos (Rust compiled to WebAssembly) app, and a Tauri desktop app. |
| 6. Confluence core | Spaces, page tree, rich-text editor, comments, and core macros. |
| 7. Migration | Import of Jira Cloud and Confluence backups. |
| 8. Launch | Jira and Confluence together. Service Management, Trello-style boards, and Statuspage follow. |
