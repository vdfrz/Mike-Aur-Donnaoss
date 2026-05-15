-- Persist citation annotations on assistant messages.
--
-- The SSE stream emits a `{"type":"citations", "citations":[...]}` event
-- at the end of each assistant turn. The frontend keeps that array
-- in-memory on `MikeMessage.annotations`, but the chat-history loader
-- (`GET /chat/:id/messages`) was only returning id/role/content — so
-- when the user re-opens an old chat from the sidebar, the inline
-- `[g1]/[p1]` citation pills can't resolve and render as plain text.
--
-- Storing the annotations as JSON on the message itself keeps the same
-- shape the frontend already speaks. NULL for messages that pre-date
-- this column (and for user/tool messages, which never carry citations).

ALTER TABLE messages ADD COLUMN annotations TEXT;
