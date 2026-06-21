-- Mark court-bundle chats so they stop leaking into the main assistant's
-- recent-chats list. The dedicated Court Bundle page sends intent="court_bundle";
-- those chat rows get kind='court_bundle' and are excluded from list_chats.
-- NULL (the default for every existing and ordinary chat) means a normal
-- assistant chat.
ALTER TABLE chats ADD COLUMN kind TEXT;
