-- this table exists to preserve the initial import data
-- given by a provider to support the ability to revert
-- a user-edited transaction back to its original state
CREATE TABLE IF NOT EXISTS imported_transaction_details (
    -- identify the transaction this is for
    transaction_id UUID PRIMARY KEY
        REFERENCES transactions(id) ON DELETE CASCADE,

    -- record the bank connection and provider the imported transaction came from
    connection_id UUID NOT NULL
        REFERENCES bank_connections(connection_id),
    provider TEXT NOT NULL,
    provider_transaction_id TEXT NOT NULL,
    pending_transaction_id TEXT,
    provider_account_id TEXT NOT NULL,

    -- preserve the original data exactly as it was received
    source_status TEXT NOT NULL
        CHECK (source_status IN ('pending', 'posted', 'removed')),
    original_payload JSONB NOT NULL,
    imported_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- preserve the initial kind, category, and description that
    -- was inferred from the initial provider data so that user
    -- edits can be replaced with their original values
    inferred_kind VARCHAR(10) NOT NULL
        CHECK (inferred_kind IN ('income', 'expense', 'transfer')),
    inferred_category TEXT,
    inferred_description TEXT
);
