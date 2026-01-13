-- create transactions table
CREATE TABLE IF NOT EXISTS transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE, -- links to users table
    amount NUMERIC(15, 2) NOT NULL,
    kind VARCHAR(10) NOT NULL CHECK (kind IN ('income', 'expense')),
    category TEXT, 
    description TEXT, -- optional fields to add more details
    date DATE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);


-- create index on user_id for faster queries
CREATE INDEX IF NOT EXISTS idx_transactions_user_id ON transactions(user_id);
-- create index based on date and category for faster date range queries
CREATE INDEX IF NOT EXISTS idx_transactions_user_date ON transactions(user_id, date);
CREATE INDEX IF NOT EXISTS idx_transactions_category ON transactions(category);