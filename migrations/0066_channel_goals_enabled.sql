-- Per-conversation goals switch. Off hides the goal tree (kind 40110) from
-- readers and rejects tree writes; the stored tree is kept for when it is
-- turned back on. Every existing conversation keeps goals on.
ALTER TABLE channels ADD COLUMN goals_enabled BOOLEAN NOT NULL DEFAULT TRUE;
