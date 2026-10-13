# Tessera

An open-source, clean-room clone of the Atlassian suite, written in Rust.

Tessera aims for 1:1 parity with Jira and Confluence: the same features, the same workflows, and the same REST APIs. Your existing scripts, clients, and integrations work after you change the server URL. Tessera runs as one binary on Linux, macOS, and Windows, and you host it yourself.

> **Status: pre-alpha.** Only the foundation exists. Do not use it for real work yet.

Tessera is not affiliated with or endorsed by Atlassian. Jira, Confluence, and Atlassian are trademarks of Atlassian.

Tessera is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE). If you run a modified Tessera as a service, you must publish your changes.

## What works today

- **JQL parser** (`crates/tessera-jql`). It parses the full public JQL grammar: fields, custom fields (`cf[10010]`), all operators, functions, lists, `EMPTY`, history predicates (`WAS`, `CHANGED`, `BY`, `DURING`, and the others), and `ORDER BY`. It reports errors with their position.
- **Server** (`crates/tessera-server`). It serves `GET /rest/api/2/serverInfo` and `GET /rest/api/3/serverInfo` in the Jira Cloud format. The official Python `jira` client connects to it with no changes.

## Run it

```bash
cargo run -p tessera-server
```

The server listens on `127.0.0.1:8080`. Set `TESSERA_ADDR` to change the address, and `TESSERA_BASE_URL` to set the public URL.

Check it with the official Python client:

```bash
uvx --with jira python -c 'from jira import JIRA; print(JIRA(server="http://127.0.0.1:8080").server_info())'
```

## Test it

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Clean-room rules

1. Build only from public sources: Atlassian's published API specs, public documentation, and the observed behavior of the products.
2. Never use leaked, decompiled, or otherwise private Atlassian code.
3. Never copy Atlassian logos, icons, or design-system assets.
4. Prove compatibility with tests. Existing Jira and Confluence clients must work against Tessera with no changes.

`scripts/fetch-specs.sh` downloads Atlassian's public OpenAPI specs into `spec/`. The specs are Atlassian's documents, so the repository does not include them.

## Roadmap

| Milestone | Scope |
| --- | --- |
| 1. Foundation | Workspace, JQL parser, server, first API compatibility check. **Done.** |
| 2. Web UI | Leptos (Rust compiled to WebAssembly) app shell, and a Tauri desktop app. |
| 3. Jira core | Projects, issues, custom fields, workflows, comments, attachments. Postgres and SQLite storage. |
| 4. JQL search | JQL compiled to SQL, saved filters, the search REST endpoints. |
| 5. Boards | Scrum and Kanban boards, sprints, backlog, and the Agile REST API. |
| 6. Confluence core | Spaces, page tree, rich-text editor, comments, and core macros. |
| 7. Migration | Import of Jira Cloud and Confluence backups. |
| 8. Launch | Jira and Confluence together. Service Management, Trello-style boards, and Statuspage follow. |
