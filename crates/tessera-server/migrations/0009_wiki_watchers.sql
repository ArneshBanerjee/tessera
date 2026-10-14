-- People who watch Confluence pages, blog posts, and spaces.
CREATE TABLE content_watchers (
    content_id  INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (content_id, user_id)
);

CREATE TABLE space_watchers (
    space_id  INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (space_id, user_id)
);
