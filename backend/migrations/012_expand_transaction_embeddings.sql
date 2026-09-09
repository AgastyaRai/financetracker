-- record which transaction revision and model produced each embedding
ALTER TABLE transaction_embeddings
ADD COLUMN search_revision INTEGER,
ADD COLUMN embedding_model TEXT;

-- backfill existing transactions as unknown
UPDATE transaction_embeddings
SET search_revision = 0,
    embedding_model = 'legacy:unknown';

-- require future embeddings to identify their transaction revision and model
ALTER TABLE transaction_embeddings
ALTER COLUMN search_revision SET NOT NULL,
ALTER COLUMN embedding_model SET NOT NULL,
ADD CONSTRAINT transaction_embeddings_search_revision_check
CHECK (search_revision >= 0);
