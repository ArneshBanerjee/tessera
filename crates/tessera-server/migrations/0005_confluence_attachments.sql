-- Confluence attachments are content of type 'attachment' whose container is a page or blog
-- post. Each version of an attachment has its own file in the data directory.
CREATE TABLE attachment_files (
    content_id  INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    media_type  TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    comment     TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (content_id, version)
);
