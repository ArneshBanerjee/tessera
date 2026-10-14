-- When a sprint was actually started. Reports measure from here, not from the planned start.
ALTER TABLE sprints ADD COLUMN activated_at INTEGER;
UPDATE sprints SET activated_at = start_date WHERE state <> 'future';
