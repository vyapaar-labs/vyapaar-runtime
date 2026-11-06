CREATE TABLE IF NOT EXISTS agent_chat_session (
    id          VARCHAR(64) PRIMARY KEY NOT NULL,
    profile     VARCHAR(64) NOT NULL,
    mode        VARCHAR(16) NOT NULL CHECK (mode IN ('live','paper','backtest')),
    title       TEXT,
    provider    VARCHAR(64) NOT NULL,
    model       VARCHAR(64) NOT NULL,
    archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_chat_session_recent ON agent_chat_session (profile, mode, archived, updated_at DESC);

CREATE TABLE IF NOT EXISTS agent_chat_message (
    session_id    VARCHAR(64) NOT NULL REFERENCES agent_chat_session(id) ON DELETE CASCADE,
    seq           BIGINT NOT NULL,
    role          VARCHAR(16) NOT NULL CHECK (role IN ('user','assistant','tool','system')),
    content       TEXT NOT NULL,
    request_hash  TEXT,
    input_tokens  INTEGER,
    output_tokens INTEGER,
    created_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_agent_chat_message_session_seq ON agent_chat_message (session_id, seq);
