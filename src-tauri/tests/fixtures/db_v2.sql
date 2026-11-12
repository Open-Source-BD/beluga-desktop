CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    model_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    text TEXT NOT NULL,
    attachments TEXT NOT NULL,
    interrupted INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_chat ON messages(chat_id, seq);
CREATE TABLE IF NOT EXISTS usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    requests INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_chat ON usage(chat_id);
CREATE INDEX IF NOT EXISTS usage_time ON usage(created_at);
ALTER TABLE chats ADD COLUMN active_skills TEXT NOT NULL DEFAULT '[]';
ALTER TABLE chats ADD COLUMN folder TEXT;
ALTER TABLE chats ADD COLUMN summary TEXT;
ALTER TABLE chats ADD COLUMN sent_from_seq INTEGER;
ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'message';
CREATE TABLE switches (
    id TEXT PRIMARY KEY NOT NULL,
    is_on INTEGER NOT NULL
);
PRAGMA user_version = 2;

INSERT INTO chats (id, title, model_id, created_at, updated_at, folder) VALUES ('chat-a', 'First chat', 'm/a', 1000, 1100, '/work/one');
INSERT INTO chats (id, title, model_id, created_at, updated_at) VALUES ('chat-b', 'Second chat', 'm/b', 2000, 2100);
INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind) VALUES ('a1', 'chat-a', 'user', 'Look at this', '[]', 0, 1001, 'message');
INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind) VALUES ('a2', 'chat-a', 'assistant', 'It says hi.', '[]', 0, 1002, 'message');
INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind) VALUES ('a3', 'chat-a', 'system', 'Skill turned on', '[]', 0, 1003, 'note');
INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind) VALUES ('b1', 'chat-b', 'user', 'Tell me a story', '[]', 0, 2001, 'message');
INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind) VALUES ('b2', 'chat-b', 'assistant', 'Once upon', '[]', 1, 2002, 'message');
INSERT INTO usage (chat_id, requests, cost_usd, created_at) VALUES ('chat-a', 1, 0.25, 1002);
INSERT INTO usage (chat_id, requests, cost_usd, created_at) VALUES ('chat-b', 2, 0.5, 2002);
