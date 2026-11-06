CREATE TABLE IF NOT EXISTS agent_chat_session (
    id          TEXT PRIMARY KEY NOT NULL,
    profile     TEXT NOT NULL,
    mode        TEXT NOT NULL CHECK (mode IN ('live','paper','backtest')),
    title       TEXT,
    provider    TEXT NOT NULL,
    model       TEXT NOT NULL,
    archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_chat_session_recent ON agent_chat_session (profile, mode, archived, updated_at DESC);

CREATE TABLE IF NOT EXISTS agent_chat_message (
    session_id    TEXT    NOT NULL REFERENCES agent_chat_session(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('user','assistant','tool','system')),
    content       TEXT    NOT NULL CHECK (json_valid(content)),
    request_hash  TEXT,
    input_tokens  INTEGER,
    output_tokens INTEGER,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_agent_chat_message_session_seq ON agent_chat_message (session_id, seq);
