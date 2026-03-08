-- create embeddings table
CREATE TABLE IF NOT EXISTS transaction_embeddings (
    transaction_id uuid PRIMARY KEY REFERENCES transactions(id) ON DELETE CASCADE, -- links to transactions
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE, -- links to user
    embedding_text text NOT NULL, -- the text we use to generate the embedding, mainly for debugging purposes
    embedding vector(1536) NOT NULL -- the actual embedding vector, 1536 dimensions for now since that's what text-embedding-3-small defaults to 
)