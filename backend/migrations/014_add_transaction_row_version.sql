ALTER TABLE transactions
    ADD COLUMN row_version BIGINT NOT NULL DEFAULT 1 CHECK (row_version > 0);
