-- Confluence page templates: global ones, space ones, and the blueprints Tessera ships.

CREATE TABLE content_templates (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    -- NULL for global templates and blueprints.
    space_id    INTEGER REFERENCES spaces (id) ON DELETE CASCADE,
    -- Storage format.
    body        TEXT    NOT NULL DEFAULT '',
    -- A JSON array of label names that pages made from the template get.
    labels      TEXT    NOT NULL DEFAULT '[]',
    -- Set for the blueprints Tessera ships, which cannot be changed.
    blueprint   TEXT,
    created_at  INTEGER NOT NULL
);

INSERT INTO content_templates (id, name, description, body, labels, blueprint, created_at) VALUES
(1, 'Meeting notes', 'Agenda, notes, and action items from a meeting.',
 '<h2>Date</h2><p>When the meeting took place</p><h2>Attendees</h2><ul><li><p>Name</p></li></ul><h2>Goals</h2><ul><li><p>What this meeting should achieve</p></li></ul><h2>Discussion</h2><table><tbody><tr><th><p>Topic</p></th><th><p>Who</p></th><th><p>Notes</p></th></tr><tr><td><p></p></td><td><p></p></td><td><p></p></td></tr></tbody></table><h2>Action items</h2><ac:task-list><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>Owner: what to do</ac:task-body></ac:task></ac:task-list>',
 '["meeting-notes"]', 'meeting-notes', 0),
(2, 'Decision', 'Record a decision, the options weighed, and why.',
 '<table><tbody><tr><th><p>Status</p></th><td><p><ac:structured-macro ac:name="status"><ac:parameter ac:name="title">Not started</ac:parameter><ac:parameter ac:name="colour">Grey</ac:parameter></ac:structured-macro></p></td></tr><tr><th><p>Stakeholders</p></th><td><p></p></td></tr><tr><th><p>Due date</p></th><td><p></p></td></tr></tbody></table><h2>Background</h2><p>What led to this decision.</p><h2>Options</h2><table><tbody><tr><th><p></p></th><th><p>Option 1</p></th><th><p>Option 2</p></th></tr><tr><th><p>Pros</p></th><td><p></p></td><td><p></p></td></tr><tr><th><p>Cons</p></th><td><p></p></td><td><p></p></td></tr></tbody></table><h2>Outcome</h2><p>What was decided, and why.</p>',
 '["decision"]', 'decision', 0),
(3, 'How-to article', 'Step-by-step instructions for a task.',
 '<p>Say in one sentence what this article helps the reader do.</p><h2>Steps</h2><ol><li><p>First step</p></li><li><p>Second step</p></li><li><p>Third step</p></li></ol><ac:structured-macro ac:name="info"><ac:rich-text-body><p>Add tips, warnings, or screenshots where they help.</p></ac:rich-text-body></ac:structured-macro><h2>Related articles</h2><ul><li><p></p></li></ul>',
 '["how-to"]', 'how-to', 0),
(4, 'Retrospective', 'Look back on a sprint or project with the team.',
 '<h2>Date</h2><p></p><h2>Team</h2><ul><li><p></p></li></ul><h2>What went well</h2><ul><li><p></p></li></ul><h2>What could be better</h2><ul><li><p></p></li></ul><h2>Action items</h2><ac:task-list><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>Owner: what to change</ac:task-body></ac:task></ac:task-list>',
 '["retrospective"]', 'retrospective', 0),
(5, 'Product requirements', 'Define a product or feature: goals, requirements, and open questions.',
 '<table><tbody><tr><th><p>Target release</p></th><td><p></p></td></tr><tr><th><p>Owner</p></th><td><p></p></td></tr><tr><th><p>Status</p></th><td><p><ac:structured-macro ac:name="status"><ac:parameter ac:name="title">Draft</ac:parameter><ac:parameter ac:name="colour">Yellow</ac:parameter></ac:structured-macro></p></td></tr></tbody></table><h2>Goals</h2><ul><li><p></p></li></ul><h2>Background</h2><p></p><h2>Requirements</h2><table><tbody><tr><th><p>#</p></th><th><p>Requirement</p></th><th><p>Importance</p></th><th><p>Notes</p></th></tr><tr><td><p>1</p></td><td><p></p></td><td><p></p></td><td><p></p></td></tr></tbody></table><h2>Questions</h2><ul><li><p></p></li></ul><h2>Not doing</h2><ul><li><p></p></li></ul>',
 '["requirements"]', 'requirements', 0);

-- Templates people make start after the blueprints.
UPDATE sqlite_sequence SET seq = 100 WHERE name = 'content_templates';
