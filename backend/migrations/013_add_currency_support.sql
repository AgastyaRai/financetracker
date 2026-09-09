-- store the currency each user chooses for totals and budgets
-- NULL means the user has not chosen one yet
ALTER TABLE users
ADD COLUMN reporting_currency TEXT;

-- preserve the exchange rate used at the time of entry for accurate conversion of old transactions
CREATE TABLE IF NOT EXISTS exchange_rate_cache (
    -- identify the currencies and requested date this rate applies to
    source_currency TEXT NOT NULL,
    reporting_currency TEXT NOT NULL,
    requested_date DATE NOT NULL,

    -- record the date and rate the provider actually supplied
    effective_date DATE NOT NULL,
    rate NUMERIC NOT NULL,
    provider TEXT NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (source_currency, reporting_currency, requested_date)
);
