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
