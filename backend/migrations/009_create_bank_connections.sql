-- create bank connections table
CREATE TABLE IF NOT EXISTS bank_connections (
    -- identify this connection internally and associate it with its owner
    connection_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- identify the external service and the connection it assigned us
    provider TEXT NOT NULL,    -- e.g. 'plaid'
    provider_id TEXT NOT NULL, -- the id the provider uses for this connection

    -- retain the encrypted credential needed for subsequent imports
    encrypted_access_token BYTEA,

    -- use this to keep track of import progress
    -- NULL means no progress has been saved yet
    progress_cursor TEXT,

    -- distinguish between imports that are active, 
    -- ones that require reauthorization, and ones that have been disconnected
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'needs_reconnection', 'disconnected')),

    -- track times of connection creation and any subsequent changes to this record
    -- updated_at must be updated when a change occurs
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- record the last successful sync, including one that returned no changes
    -- NULL before the first success, failed attempts leave this unchanged
    last_synced_at TIMESTAMPTZ,

    -- permit credential removal on disconnect while retaining imported history
    CHECK (status = 'disconnected' OR encrypted_access_token IS NOT NULL),

    -- prevent storing the same external connection more than once
    UNIQUE (provider, provider_id),

    -- support a composite foreign key that checks source-revision ownership
    UNIQUE (connection_id, user_id)
);

-- index on user id to allow us to more efficiently find bank
-- connections belonging to a specific user
CREATE INDEX IF NOT EXISTS idx_bank_connections_user_id
ON bank_connections(user_id);
