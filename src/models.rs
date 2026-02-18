use sqlx::types::Decimal;

/* data structures */

// struct to hold shared application state
#[derive(Clone)]
pub struct AppState {
    // database connection pool
    pub pool: sqlx::PgPool,
    // jwt_secret: String, secret key for signing JWTs
    pub jwt_secret: String,
}

// struct for user registration
#[derive(serde::Deserialize)]
pub(crate) struct RegisterUser {
    pub username: String,
    pub email: String,
    pub password: String,
}

// structs for user login
#[derive(serde::Deserialize)]
pub(crate) struct LoginUser {
    pub identifier: String, // can be username or email
    pub password: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct LoginResponse {
    pub user_id: uuid::Uuid,
    pub access_token: String, // for JWT authentication
}

// enum for transaction kind
#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) enum TransactionKind {
    Income,
    Expense,
}

// struct for adding a transaction (request body - no user_id)
#[derive(serde::Deserialize)]
pub(crate) struct AddTransactionRequest {
    pub amount: Decimal,
    pub kind: TransactionKind,
    pub category: Option<String>,
    pub date: chrono::NaiveDate,
    pub description: Option<String>,
}

// struct for transaction response
#[derive(serde::Serialize)]
pub(crate) struct Transaction {
    pub user_id: uuid::Uuid,
    pub amount: Decimal,
    pub kind: TransactionKind,
    pub category: Option<String>,
    pub date: chrono::NaiveDate,
    pub description: Option<String>,
}

// struct for adding/updating a budget (request body - no user_id)
#[derive(serde::Deserialize)]
pub(crate) struct UpsertBudgetRequest {
    pub month: chrono::NaiveDate, // first day of month (e.g., 2026-01-01)
    pub category: String,
    pub amount: Decimal,
}

// struct for budget response
#[derive(serde::Serialize)]
pub(crate) struct Budget {
    pub user_id: uuid::Uuid,
    pub month: chrono::NaiveDate,
    pub category: String,
    pub amount: Decimal,
}

// query params for budgets (optional month filter)
#[derive(serde::Deserialize)]
pub(crate) struct BudgetQuery {
    pub month: Option<chrono::NaiveDate>,
}

// struct for returning budget progress (budget vs spent)
#[derive(serde::Serialize)]
pub(crate) struct BudgetProgress {
    pub category: String,
    pub budget_amount: Decimal,
    pub spent: Decimal,
    pub remaining: Decimal,
}

// struct for JWT claims
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Claims {
    pub sub: String, // we store the user ID as a string in the JWT claims
    pub exp: usize, // expiration time as a unix timestamp
}

// struct for an authenticated user (for extracting user ID from JWT in protected routes)
pub(crate) struct AuthenticatedUser {
    pub user_id: uuid::Uuid,
}

/* constants */

pub(crate) const JWT_EXPIRATION_HOURS: i64 = 24; // JWT expiration time in hours
