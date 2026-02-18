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
- `POST /users/register`
- `POST /users/login`
- `POST /transactions`
- `GET  /transactions/:user_id`
- `POST /budgets` (upsert)
- `GET  /budgets/:user_id`
- `GET  /budgets/:user_id/progress`
- `GET  /test` (development)

## Local Development

### 1) Backend
Create a `.env` file (or export env vars) with:

- `DATABASE_URL=postgresql://...` (Supabase connection string; include `?sslmode=require` if needed)
- `PORT=3000` (optional; defaults to 3000)

Run migrations:
```bash
cargo sqlx migrate run --source backend/migrations
