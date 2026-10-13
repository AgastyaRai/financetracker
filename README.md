# FinanceTracker

Live demo: https://financetracker-9wbe.onrender.com/  

FinanceTracker is a full-stack personal finance web app that lets users register/login, add income/expense transactions, set monthly category budgets, and view basic spending/budget analytics.

## Features
- **User authentication** (register + login) with **Argon2 password hashing**
- **Transactions**: add & view income/expense entries (amount, category, date, description)
- **Budgets**: upsert monthly budgets by category
- **Analytics**: budget progress (spent vs remaining) computed server-side via SQL aggregation
- **Deployed**: frontend + backend hosted on Render, database on Supabase Postgres

## Tech Stack
**Backend**
- Rust, Axum (REST API)
- SQLx (typed queries + migrations)
- PostgreSQL (Supabase)
- Argon2 (password hashing)
- Serde, Chrono, UUID, Decimal

**Frontend**
- Vite + (React) frontend
- Fetch-based API client
- Environment-based API base URL

**Deployment**
- Render (web service + static site)
- Supabase Postgres (with connection pooler/TLS)

## API Routes (summary)
All routes below are served under `/api`.

- `POST /users/register`
- `POST /users/login`
- `POST /transactions`
- `GET  /transactions`
- `PUT  /transactions/:id`
- `POST /transactions/search/semantic`
- `POST /budgets` (upsert)
- `GET  /budgets`
- `GET  /budgets/progress`
- `GET  /test` (development)

## Local Development

### 1) Backend
Create a `.env` file (or export env vars) with:

- `DATABASE_URL=postgresql://...` (Supabase connection string; include `?sslmode=require` if needed)
- `PORT=3000` (optional; defaults to 3000)

Run migrations:
```bash
cargo sqlx migrate run --source backend/migrations
```

For local development in this repository, copy the safe example file:

```powershell
Copy-Item backend/.env.example backend/.env
```

The backend loads `backend/.env`. Set the following values before starting it:

- `DATABASE_URL` points to the local PostgreSQL database. The provided Docker setup uses host port `5433`.
- `JWT_SECRET` is a private secret used to sign authentication tokens.
- `OPENAI_API_KEY` is a private server-side OpenAI API key.
- `PORT` is optional and defaults to `3000`.
- `RUN_MIGRATIONS` is optional; when set, the backend applies migrations during startup.

Never commit `backend/.env` or place `OPENAI_API_KEY` in frontend code. Only `backend/.env.example`, which contains placeholder values, should be tracked.


## Deployment (Render + Supabase)

Apply schema migrations to the Supabase database used by Render's `DATABASE_URL` before a new backend release serves requests. This branch requires `009_create_bank_connections.sql` and `011_create_imported_transaction_details.sql` for transaction loading and imported-field Revert.

To run the repository's migrations during startup, set `RUN_MIGRATIONS=1` on the **Render backend service**, then redeploy it. SQLx applies pending migrations before the HTTP server starts and records completed versions, so subsequent startups do not reapply them. The environment variable can remain enabled for later releases. Verify the database's SQLx migration history before enabling this on a database whose schema was previously managed manually.

When migrations are managed separately, run `cargo sqlx migrate run --source backend/migrations` against that same database before deployment. Startup now checks for the required import tables and reports migration instructions instead of starting a server whose transaction routes fail. The migrations are versioned in the repository; do not rewrite already-applied migration files.


Supabase Data API access is separate from this application's Rust API. FinanceTracker uses its own users/JWTs and a privileged PostgreSQL backend connection, so Supabase `anon`/`authenticated` roles must not receive access to application rows. Enable RLS without Data API policies on application tables, including import provenance and SQLx history, and disable **Automatically expose new tables** in Supabase's Data API settings. Verify the backend connection's access before changing RLS when provisioning a different deployment.

SQLx checksums include migration file line endings. If an existing database was migrated from both Windows and Linux, inspect its recorded checksums against the exact repository bytes before enabling startup migrations. Reconcile only proven CRLF/LF differences through a reviewed metadata repair; never ignore a checksum mismatch or overwrite migration history without establishing its cause. The deployed Supabase database now matches the Linux/Git migration checksums, and the original migration files remain unchanged.


For automatic deployments from GitHub, install Render's GitHub app with access scoped to the FinanceTracker repository, confirm the service's Git credentials and deployed branch, and select **On Commit**. A service can still clone and deploy manually without receiving push events when this integration is missing. Verify the connection by checking that a real branch push creates an automatic deployment; the Auto-Deploy setting alone is not that verification.


For the reviewed private Rust API deployment, apply `backend/deployment/supabase_security.sql` as `postgres` in one transaction after the SQLx migrations. This Supabase-specific configuration is separate from application schema migration history. It enables RLS on the eight application tables, revokes their `anon`/`authenticated`/`PUBLIC` object grants to hide GraphQL schema metadata, and removes all automatic table and sequence privileges for those roles on future objects created by `postgres` in `public`. Other tables, schemas, policies, and server roles are not changed.

The same file moves the existing pgvector extension into `extensions`, preserving vector type and index identities. Backend connections must already include both `public` and `extensions` in their `search_path`; verify this on the backend connection before applying the file. The setup refuses a different table owner, unexpected vector location, missing extension search path, or existing application Data API policies so those access models can be reviewed first. It is safe to repeat for this deployment. With `psql`, use `--single-transaction --set ON_ERROR_STOP=1 --file backend/deployment/supabase_security.sql` against the same database used by the backend.

After applying the setup, verify that the Supabase Security Advisor has no RLS-disabled, GraphQL table-exposure, or extension-in-public warnings and that the backend can still load transactions and store/search embeddings. RLS-without-policy informational notices are intentional for this custom backend ownership model; do not add public policies just to silence them. The disposable `supabase_security_tests` cover preserved records, fresh-connection vector writes and search, restricted existing and future Data API grants, repeatability, and rollback on incompatible configurations.
