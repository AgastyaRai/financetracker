-- add a HNSW index on the transaction_embeddings table for efficient vector retrieval
CREATE INDEX IF NOT EXISTS transaction_embeddings_embedding_idx 
    ON transaction_embeddings
    USING hnsw (embedding vector_l2_ops);