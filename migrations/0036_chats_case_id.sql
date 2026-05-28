-- Tag case-scoped chats so they don't appear in the assistant's recent-chats list.
ALTER TABLE chats ADD COLUMN case_id TEXT REFERENCES cases(id) ON DELETE CASCADE;
