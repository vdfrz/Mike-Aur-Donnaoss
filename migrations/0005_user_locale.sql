-- Persist UI locale preference per-user so the language follows the data
-- folder (security/portability requirement). The frontend syncs the cookie
-- used for SSR with this value on profile load and on language-switcher
-- writes.

ALTER TABLE user_settings ADD COLUMN locale TEXT;
