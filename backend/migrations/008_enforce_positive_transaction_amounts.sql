-- enforce positive amounts for all new or updated transactions without blocking deployment on unknown legacy data
ALTER TABLE transactions
ADD CONSTRAINT transactions_amount_positive
CHECK (amount > 0)
NOT VALID;

-- validate the constraint immediately when existing data already satisfies it
-- if legacy invalid rows exist, new writes are still protected and the constraint can be validated after cleanup
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM transactions
        WHERE amount <= 0
    ) THEN
        ALTER TABLE transactions
        VALIDATE CONSTRAINT transactions_amount_positive;
    END IF;
END
$$;
