-- Confluence: spaces, pages, blog posts, comments, their versions, and labels.

CREATE TABLE spaces (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    key          TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    name         TEXT    NOT NULL,
    -- Plain text, as Confluence stores space descriptions.
    description  TEXT    NOT NULL DEFAULT '',
    -- 'global' or 'personal'.
    type         TEXT    NOT NULL DEFAULT 'global',
    -- 'current' or 'archived'.
    status       TEXT    NOT NULL DEFAULT 'current',
    -- Set once the home page exists. Cleared if it is purged.
    homepage_id  INTEGER REFERENCES content (id) ON DELETE SET NULL,
    creator_id   INTEGER NOT NULL REFERENCES users (id),
    created_at   INTEGER NOT NULL
);

-- Pages, blog posts, and comments share one table, as they share one id space in Confluence.
CREATE TABLE content (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 'page', 'blogpost', or 'comment'.
    type            TEXT    NOT NULL,
    -- 'current', 'draft', or 'trashed'.
    status          TEXT    NOT NULL DEFAULT 'current',
    space_id        INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    -- Pages: the parent page. Comments: the comment this one replies to.
    parent_id       INTEGER REFERENCES content (id) ON DELETE SET NULL,
    -- Comments: the page or blog post they are on.
    container_id    INTEGER REFERENCES content (id) ON DELETE CASCADE,
    title           TEXT    NOT NULL,
    -- The current version's body, in storage format (XHTML).
    body            TEXT    NOT NULL DEFAULT '',
    -- A plain-text copy of the body, for CQL text search.
    body_text       TEXT    NOT NULL DEFAULT '',
    version         INTEGER NOT NULL DEFAULT 1,
    -- Order among sibling pages. Lower comes first.
    position        INTEGER NOT NULL DEFAULT 0,
    creator_id      INTEGER NOT NULL REFERENCES users (id),
    created_at      INTEGER NOT NULL,
    updated_by      INTEGER NOT NULL REFERENCES users (id),
    updated_at      INTEGER NOT NULL
);
CREATE INDEX content_space ON content (space_id, type, status);
CREATE INDEX content_parent ON content (parent_id);
CREATE INDEX content_container ON content (container_id);

-- Every version of every piece of content, the current one included.
CREATE TABLE content_versions (
    content_id  INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    number      INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    body        TEXT    NOT NULL,
    author_id   INTEGER NOT NULL REFERENCES users (id),
    created_at  INTEGER NOT NULL,
    message     TEXT    NOT NULL DEFAULT '',
    minor_edit  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (content_id, number)
);

-- Labels are shared: the same label on two pages has one id.
CREATE TABLE labels (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 'global', 'my', or 'team'.
    prefix  TEXT NOT NULL DEFAULT 'global',
    name    TEXT NOT NULL,
    UNIQUE (prefix, name)
);

CREATE TABLE content_labels (
    content_id  INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    label_id    INTEGER NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (content_id, label_id)
);

-- Confluence ids are large numbers. Start ours in the same range.
INSERT INTO sqlite_sequence (name, seq) VALUES
    ('spaces', 98303), ('content', 65535), ('labels', 0);
