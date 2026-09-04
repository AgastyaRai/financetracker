-- replace the L2 HNSW index because semantic search queries use cosine distance
DROP INDEX IF EXISTS transaction_embeddings_embedding_idx;

-- recreate the HNSW index with the operator class used by the <=> cosine distance query
CREATE INDEX IF NOT EXISTS transaction_embeddings_embedding_idx
    ON transaction_embeddings
    USING hnsw (embedding vector_cosine_ops);
