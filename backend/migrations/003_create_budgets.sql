-- create budgets table
CREATE TABLE IF NOT EXISTS budgets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE, -- links to users table
    month DATE NOT NULL, -- store as first day of the month (e.g., 2026-01-01)
    category TEXT NOT NULL, -- budget category (e.g., "food", "gym")
    amount NUMERIC(15, 2) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- ensure one budget per user/category/month
CREATE UNIQUE INDEX IF NOT EXISTS uniq_budgets_user_month_category
ON budgets(user_id, month, category);

-- create index on user_id for faster queries
CREATE INDEX IF NOT EXISTS idx_budgets_user_id ON budgets(user_id);

-- create index on month for faster month queries
CREATE INDEX IF NOT EXISTS idx_budgets_user_month ON budgets(user_id, month);
