-- Inline comments: comments on a piece of a page's text. The page body wraps that text in an
-- <ac:inline-comment-marker ac:ref="..."> with the comment's marker reference.

CREATE TABLE inline_comments (
    content_id         INTEGER PRIMARY KEY REFERENCES content (id) ON DELETE CASCADE,
    -- Replies share the marker of the comment they answer.
    marker_ref         TEXT    NOT NULL,
    original_selection TEXT    NOT NULL DEFAULT '',
    -- 'open', 'reopened', or 'resolved'. A comment whose marker left the page is 'dangling';
    -- that is worked out from the page body when read.
    resolution         TEXT    NOT NULL DEFAULT 'open',
    resolution_by      INTEGER REFERENCES users (id) ON DELETE SET NULL,
    resolution_at      INTEGER
);

CREATE INDEX inline_comments_marker ON inline_comments (marker_ref);
