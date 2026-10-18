-- Page restrictions, as in Confluence. A 'read' restriction limits who may view the page and
-- every page below it; an 'update' restriction limits who may edit the page itself.
CREATE TABLE content_restrictions (
    content_id    INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    -- 'read' or 'update'.
    operation     TEXT    NOT NULL,
    -- 'user' (subject: account id) or 'group' (subject: group name).
    subject_type  TEXT    NOT NULL,
    subject       TEXT    NOT NULL,
    PRIMARY KEY (content_id, operation, subject_type, subject)
);
