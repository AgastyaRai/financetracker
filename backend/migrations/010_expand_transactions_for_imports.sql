-- we need to expand the transactions table so
-- that it can properly represent imported transactions

-- store money entering an account as positive and
-- money leaving an account as negative
ALTER TABLE transactions
DROP CONSTRAINT transactions_amount_positive,
ADD CONSTRAINT transactions_amount_nonzero
CHECK (amount <> 0)
NOT VALID;

-- existing expenses represent money leaving the account
UPDATE transactions
SET amount = -amount
WHERE kind = 'expense' AND amount > 0;

-- validate the new rule if there are no existing zero-value transactions
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM transactions
        WHERE amount = 0
    ) THEN
        ALTER TABLE transactions
        VALIDATE CONSTRAINT transactions_amount_nonzero;
    END IF;
END
$$;

-- allow transfers as a new type 
-- in addition to income and expenses
ALTER TABLE transactions
DROP CONSTRAINT transactions_kind_check,
ADD CONSTRAINT transactions_kind_check
CHECK (kind IN ('income', 'expense', 'transfer'));

-- every transaction must belong to a user
ALTER TABLE transactions
ALTER COLUMN user_id SET NOT NULL;

-- distinguish manually entered transactions from imported ones
-- and retain the current state of imported transactions
ALTER TABLE transactions
ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual'
    CHECK (origin IN ('manual', 'imported')),
ADD COLUMN status TEXT NOT NULL DEFAULT 'posted'
    CHECK (status IN ('pending', 'posted', 'removed')),
ADD COLUMN currency TEXT;

-- preserve the creation time as the initial update time for existing transactions
ALTER TABLE transactions
ADD COLUMN updated_at TIMESTAMPTZ;

UPDATE transactions
SET updated_at = created_at;

ALTER TABLE transactions
ALTER COLUMN updated_at SET NOT NULL,
ALTER COLUMN updated_at SET DEFAULT CURRENT_TIMESTAMP;

-- track which imported values have been replaced by the user
ALTER TABLE transactions
ADD COLUMN kind_overridden BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN category_overridden BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN description_overridden BOOLEAN NOT NULL DEFAULT FALSE;

-- gets incremented when kind, category, or description changes
-- allows us to track whether an embedding is based on the current transaction values
ALTER TABLE transactions
ADD COLUMN search_revision INTEGER NOT NULL DEFAULT 1
    CHECK (search_revision > 0);
